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
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
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
    runtime::{LogLine, LogRequest, Runtime, RuntimeEvent, Source},
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
    /// IDs explicitly destroyed: their followers must not resurrect cursors.
    forgotten: Arc<Mutex<HashSet<Key>>>,
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
            forgotten: Arc::default(),
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
                let running: HashSet<&str> =
                    containers.iter().map(|meta| meta.id.as_str()).collect();
                // A container missing from the snapshot stopped while the
                // event stream was down: stop following it instead of relying
                // on a `die` event that was never delivered.
                self.followers.retain(|(_, id), f| {
                    if running.contains(id.as_str()) {
                        true
                    } else {
                        f.cancel.cancel();
                        false
                    }
                });
                for meta in containers {
                    self.up(meta, true);
                }
            }
            RuntimeEvent::Up(meta) => self.up(meta, false),
            RuntimeEvent::Down { id, removed } => {
                if removed {
                    let keys: Vec<Key> = self
                        .followers
                        .keys()
                        .filter(|(_, cid)| *cid == id)
                        .cloned()
                        .collect();
                    {
                        let mut forgotten = self.forgotten.lock().unwrap();
                        let mut cursors = self.cursors.lock().unwrap();
                        for key in &keys {
                            forgotten.insert(key.clone());
                            cursors.remove(key);
                        }
                    }
                    for key in keys {
                        self.followers.remove(&key).unwrap().cancel.cancel();
                    }
                }
            }
        }
    }

    fn is_self(&self, meta: &ContainerMeta) -> bool {
        self.self_id
            .as_deref()
            .is_some_and(|own| !own.is_empty() && meta.id.starts_with(own))
    }

    fn up(&mut self, meta: ContainerMeta, from_snapshot: bool) {
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
                        // Snapshots are idempotent: replaying them must not
                        // make a healthy follower reconnect needlessly.
                        if !from_snapshot {
                            state.restarted = true;
                        }
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
            self.forgotten.lock().unwrap().remove(&key);
            let task = FollowerTask {
                runtime: self.runtime.clone(),
                sink: sink.clone(),
                key: key.clone(),
                baseline: sink.options.since.baseline(self.started_at),
                cursors: self.cursors.clone(),
                forgotten: self.forgotten.clone(),
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
    forgotten: Arc<Mutex<HashSet<Key>>>,
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

    fn line_hash(line: &LogLine) -> u64 {
        let mut hasher = DefaultHasher::new();
        line.source.hash(&mut hasher);
        line.data.hash(&mut hasher);
        hasher.finish()
    }

    async fn follow_once(&self) -> Ended {
        let cursor = self
            .cursors
            .lock()
            .unwrap()
            .get(&self.key)
            .cloned()
            .unwrap_or_else(|| Cursor::new(self.baseline));
        let since = cursor.ts;
        let mut replay = Replay::new(cursor);
        let options = &self.sink.options;
        let request = LogRequest {
            stdout: options.stdout,
            stderr: options.stderr,
            since,
            max_line_bytes: options.max_line_bytes,
        };
        // Render prefixes once per metadata generation instead of once per line.
        let mut prefix_meta = self.meta();
        let mut prefixes = render_prefixes(options, &prefix_meta);
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

            let current_meta = self.meta();
            if !Arc::ptr_eq(&current_meta, &prefix_meta) {
                prefix_meta = current_meta;
                prefixes = render_prefixes(options, &prefix_meta);
            }

            let received = SystemTime::now();
            // Lines without a usable runtime timestamp are positioned at
            // reception time so they can be deduplicated after a reconnect.
            let effective_ts = line.ts.unwrap_or(received);
            if !replay.accept(effective_ts, Self::line_hash(&line)) {
                continue;
            }

            let time = match options.timestamp {
                TimestampMode::Receive => received,
                TimestampMode::Runtime => effective_ts,
            };
            let time = time.duration_since(UNIX_EPOCH).unwrap_or_default();

            let data = to_text(&line.data);
            let text = match (&prefixes, line.source) {
                (None, _) => data,
                (Some((stdout, _)), Source::Stdout) => {
                    let mut text = stdout.clone();
                    text.push_str(&data);
                    text
                }
                (Some((_, stderr)), Source::Stderr) => {
                    let mut text = stderr.clone();
                    text.push_str(&data);
                    text
                }
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
            let forgotten = self.forgotten.lock().unwrap();
            if !forgotten.contains(&self.key) {
                self.cursors
                    .lock()
                    .unwrap()
                    .insert(self.key.clone(), replay.cursor());
            }
        }
    }
}

fn render_prefixes(options: &StreamOptions, meta: &ContainerMeta) -> Option<(String, String)> {
    if options.prefix.is_empty() {
        return None;
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    options.prefix.render(meta, Source::Stdout, &mut stdout);
    options.prefix.render(meta, Source::Stderr, &mut stderr);
    Some((stdout, stderr))
}
