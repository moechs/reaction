//! Discovery, reconciliation and log following, independent of the runtime.
//!
//! One [`Supervisor`] runs per backend. It consumes [`Runtime::watch`] and keeps
//! exactly one follower task per (stream, container) pair that matches.
//!
//! Invariants:
//! - The supervisor and its followers hold clones of each stream's `Sender`
//!   until shutdown. reaction treats a closed plugin stream as an exited
//!   stream (plugin streams have no `eof`/restart handling), so a stream must
//!   stay open even while no container matches.
//! - Cursors outlive followers, so a restarted container resumes where its
//!   previous follower stopped. They are dropped when the container is removed.
//! - A follower only exits on its own when the runtime says the container is
//!   no longer running, and that decision is serialized with the supervisor
//!   through [`FollowerState`] so a quick restart can never be missed.

use std::{
    collections::HashMap,
    pin::pin,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::StreamExt;
use reaction_plugin::{Line, shutdown::ShutdownToken};
use remoc::rch::mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    config::{StreamOptions, TimestampMode},
    cursor::{Cursor, Replay},
    lines::to_text,
    meta::ContainerMeta,
    runtime::{LogRequest, Runtime, RuntimeEvent},
};

/// One reaction stream fed by this backend.
pub struct Sink {
    pub stream_name: String,
    pub options: StreamOptions,
    pub tx: mpsc::Sender<Line>,
}

type Key = (usize, String);
type Cursors = Arc<Mutex<HashMap<Key, Cursor>>>;

/// Shared between the supervisor and one follower.
struct FollowerState {
    /// Latest metadata (refreshed on rename), used for prefixes and messages.
    meta: Arc<ContainerMeta>,
    /// False once the follower decided to exit.
    alive: bool,
    /// Set by the supervisor when the container was reported running again
    /// while the follower was alive: the follower must reconnect instead of exiting.
    restarted: bool,
}

struct Follower {
    state: Arc<Mutex<FollowerState>>,
    cancel: CancellationToken,
}

pub struct Supervisor<R: Runtime> {
    runtime: Arc<R>,
    sinks: Vec<Arc<Sink>>,
    reconnect_delay: Duration,
    started_at: SystemTime,
    self_id: Option<String>,
    shutdown: ShutdownToken,
    followers: HashMap<Key, Follower>,
    cursors: Cursors,
}

impl<R: Runtime> Supervisor<R> {
    pub fn new(
        runtime: Arc<R>,
        sinks: Vec<Sink>,
        reconnect_delay: Duration,
        started_at: SystemTime,
        shutdown: ShutdownToken,
    ) -> Self {
        let self_id = runtime.self_id();
        if let Some(id) = &self_id {
            log_debug!("{}: own container {id} will be ignored", runtime.name());
        }
        Self {
            runtime,
            sinks: sinks.into_iter().map(Arc::new).collect(),
            reconnect_delay,
            started_at,
            self_id,
            shutdown,
            followers: HashMap::new(),
            cursors: Arc::default(),
        }
    }

    pub async fn run(mut self) {
        let shutdown = self.shutdown.clone();
        loop {
            {
                let mut events = pin!(self.runtime.watch());
                loop {
                    let item = tokio::select! {
                        _ = shutdown.wait() => return,
                        item = events.next() => item,
                    };
                    match item {
                        Some(Ok(event)) => self.handle(event),
                        Some(Err(err)) => {
                            log_warn!("{}: watching containers: {err}", self.runtime.name());
                            break;
                        }
                        None => {
                            log_warn!("{}: event stream ended", self.runtime.name());
                            break;
                        }
                    }
                }
            }
            tokio::select! {
                _ = shutdown.wait() => return,
                _ = tokio::time::sleep(self.reconnect_delay) => {}
            }
        }
    }

    fn handle(&mut self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::Snapshot(containers) => {
                self.followers.retain(|_, f| f.state.lock().unwrap().alive);
                for meta in containers {
                    self.up(meta);
                }
            }
            RuntimeEvent::Up(meta) => self.up(meta),
            RuntimeEvent::Down { id, removed } => {
                if removed {
                    self.cursors
                        .lock()
                        .unwrap()
                        .retain(|(_, cid), _| *cid != id);
                    self.followers
                        .retain(|(_, cid), f| *cid != id || f.state.lock().unwrap().alive);
                }
            }
        }
    }

    fn is_self(&self, meta: &ContainerMeta) -> bool {
        self.self_id
            .as_deref()
            .is_some_and(|own| !own.is_empty() && meta.id.starts_with(own))
    }

    fn up(&mut self, meta: ContainerMeta) {
        if self.is_self(&meta) {
            return;
        }
        let meta = Arc::new(meta);
        for (index, sink) in self.sinks.iter().enumerate() {
            let key = (index, meta.id.clone());
            let wanted = sink.options.filter.matches(&meta);

            if let Some(follower) = self.followers.get(&key) {
                let mut state = follower.state.lock().unwrap();
                if state.alive {
                    if wanted {
                        state.restarted = true;
                        state.meta = meta.clone();
                    } else {
                        drop(state);
                        log_info!(
                            "stream {}: container {} no longer matches, stop following",
                            sink.stream_name,
                            meta.name
                        );
                        follower.cancel.cancel();
                        self.followers.remove(&key);
                    }
                    continue;
                }
            }
            if !wanted {
                continue;
            }

            log_info!(
                "stream {}: following {} container {} ({})",
                sink.stream_name,
                self.runtime.name(),
                meta.name,
                meta.short_id()
            );
            let state = Arc::new(Mutex::new(FollowerState {
                meta: meta.clone(),
                alive: true,
                restarted: false,
            }));
            let cancel = CancellationToken::new();
            let task = FollowerTask {
                runtime: self.runtime.clone(),
                sink: sink.clone(),
                key: key.clone(),
                baseline: sink.options.since.baseline(self.started_at),
                cursors: self.cursors.clone(),
                reconnect_delay: self.reconnect_delay,
                state: state.clone(),
                cancel: cancel.clone(),
                shutdown: self.shutdown.clone(),
            };
            tokio::spawn(task.run());
            self.followers.insert(key, Follower { state, cancel });
        }
    }
}

struct FollowerTask<R: Runtime> {
    runtime: Arc<R>,
    sink: Arc<Sink>,
    /// (sink index, container id)
    key: Key,
    baseline: SystemTime,
    cursors: Cursors,
    reconnect_delay: Duration,
    state: Arc<Mutex<FollowerState>>,
    cancel: CancellationToken,
    shutdown: ShutdownToken,
}

enum Ended {
    /// Stream ended or failed: maybe reconnect.
    Stream(Option<String>),
    /// Shutdown, cancellation or reaction closed the stream: exit now.
    Stop,
}

impl<R: Runtime> FollowerTask<R> {
    fn id(&self) -> &str {
        &self.key.1
    }

    fn meta(&self) -> Arc<ContainerMeta> {
        self.state.lock().unwrap().meta.clone()
    }

    async fn run(self) {
        loop {
            let error = match self.follow_once().await {
                Ended::Stop => return,
                Ended::Stream(error) => error,
            };
            if self.should_exit(error).await {
                return;
            }
            tokio::select! {
                _ = self.shutdown.wait() => return,
                _ = self.cancel.cancelled() => return,
                _ = tokio::time::sleep(self.reconnect_delay) => {}
            }
        }
    }

    /// Decide whether the container is gone for good, after its log stream ended.
    async fn should_exit(&self, error: Option<String>) -> bool {
        let running = tokio::select! {
            _ = self.shutdown.wait() => return true,
            _ = self.cancel.cancelled() => return true,
            running = self.runtime.is_running(self.id()) => running,
        };
        match running {
            Ok(true) => {
                if let Some(err) = error {
                    log_warn!(
                        "stream {}: reading logs of {}: {err}",
                        self.sink.stream_name,
                        self.meta().name
                    );
                }
                false
            }
            Ok(false) => {
                let mut state = self.state.lock().unwrap();
                if state.restarted {
                    state.restarted = false;
                    false
                } else {
                    state.alive = false;
                    log_info!(
                        "stream {}: container {} stopped",
                        self.sink.stream_name,
                        state.meta.name
                    );
                    true
                }
            }
            Err(err) => {
                log_warn!(
                    "stream {}: checking container {}: {err}",
                    self.sink.stream_name,
                    self.meta().name
                );
                false
            }
        }
    }

    async fn follow_once(&self) -> Ended {
        let cursor = self
            .cursors
            .lock()
            .unwrap()
            .get(&self.key)
            .copied()
            .unwrap_or_else(|| Cursor::new(self.baseline));
        let mut replay = Replay::new(cursor);
        let options = &self.sink.options;
        let request = LogRequest {
            stdout: options.stdout,
            stderr: options.stderr,
            since: cursor.ts,
            max_line_bytes: options.max_line_bytes,
        };
        let mut lines = pin!(self.runtime.logs(self.id(), request));

        loop {
            let line = tokio::select! {
                _ = self.shutdown.wait() => return Ended::Stop,
                _ = self.cancel.cancelled() => return Ended::Stop,
                line = lines.next() => line,
            };
            let line = match line {
                Some(Ok(line)) => line,
                Some(Err(err)) => return Ended::Stream(Some(err)),
                None => return Ended::Stream(None),
            };
            if !replay.accept(line.ts) {
                continue;
            }

            let received = SystemTime::now();
            let time = match options.timestamp {
                TimestampMode::Receive => received,
                TimestampMode::Runtime => line.ts.unwrap_or(received),
            };
            let time = time.duration_since(UNIX_EPOCH).unwrap_or_default();

            let text = if options.prefix.is_empty() {
                to_text(&line.data)
            } else {
                let mut text = String::new();
                options.prefix.render(&self.meta(), line.source, &mut text);
                text.push_str(&to_text(&line.data));
                text
            };

            let sent = tokio::select! {
                _ = self.shutdown.wait() => return Ended::Stop,
                _ = self.cancel.cancelled() => return Ended::Stop,
                sent = self.sink.tx.send((text, time)) => sent,
            };
            if let Err(err) = sent {
                if !self.shutdown.is_shutdown() {
                    log_error!(
                        "stream {}: cannot send line to reaction: {err}",
                        self.sink.stream_name
                    );
                }
                return Ended::Stop;
            }
            self.cursors
                .lock()
                .unwrap()
                .insert(self.key.clone(), replay.cursor());
        }
    }
}
