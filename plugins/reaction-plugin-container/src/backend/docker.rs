//! Docker Engine API backend (stream type `docker`), built on bollard.
//!
//! - Discovery: `GET /containers/json` for the snapshot, then
//!   `GET /events` filtered on container `start`/`die`/`destroy`/`rename`.
//!   Events are requested with a `since` slightly before the snapshot, so
//!   nothing that happens between the two requests is missed (replayed events
//!   are harmless: the supervisor is idempotent).
//! - Logs: `GET /containers/{id}/logs?follow=1&timestamps=1&since=<secs>`.
//!   bollard decodes the multiplexed (non-TTY) and raw (TTY) formats into
//!   chunks; every chunk starts with Docker's RFC 3339 timestamp, which is
//!   stripped here. Lines are reassembled per output by [`LineAssembler`].

use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bollard::{
    Docker,
    container::LogOutput,
    errors::Error as DockerError,
    models::{ContainerInspectResponse, ContainerSummary, EventMessage},
    query_parameters::{EventsOptionsBuilder, ListContainersOptionsBuilder, LogsOptionsBuilder},
};
use futures::{Stream, StreamExt, stream};
use serde::Deserialize;

use crate::core::{
    lines::LineAssembler,
    meta::ContainerMeta,
    runtime::{LogLine, LogRequest, Runtime, RuntimeEvent, Source},
};

pub const STREAM_TYPE: &str = "docker";
/// Backend-specific template variables (none for Docker).
pub const EXTRA_KEYS: &[&str] = &[];

/// Events older than the snapshot by this much are replayed, to cover clock
/// skew between the plugin and a remote daemon.
const EVENTS_REPLAY_MARGIN: Duration = Duration::from_secs(5);

/// `plugins.<name>.options.docker`
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockerOptions {
    /// `unix:///path/to/docker.sock`, `tcp://host:port` or `http://host:port`.
    /// Defaults to `$DOCKER_HOST`, then `unix:///var/run/docker.sock`.
    #[serde(default)]
    pub host: Option<String>,
}

impl DockerOptions {
    pub fn validate(&self) -> Result<(), String> {
        match &self.host {
            None => Ok(()),
            Some(host)
                if host.starts_with("unix://")
                    || host.starts_with("tcp://")
                    || host.starts_with("http://") =>
            {
                Ok(())
            }
            Some(host) => Err(format!(
                "docker.host {host:?}: only unix://, tcp:// and http:// are supported"
            )),
        }
    }
}

pub struct DockerRuntime {
    docker: Docker,
    self_id: Option<String>,
}

impl DockerRuntime {
    /// Create the client, negotiate the API version and ping the daemon.
    pub async fn connect(options: &DockerOptions) -> Result<Self, String> {
        let docker = match &options.host {
            Some(host) => Docker::connect_with_host(host),
            None => Docker::connect_with_defaults(),
        }
        .map_err(|err| format!("cannot create docker client: {err}"))?;
        let docker = docker
            .negotiate_version()
            .await
            .map_err(|err| format!("cannot reach docker daemon: {err}"))?;
        docker
            .ping()
            .await
            .map_err(|err| format!("cannot ping docker daemon: {err}"))?;

        let self_id = std::fs::read_to_string("/proc/self/mountinfo")
            .ok()
            .and_then(|mountinfo| parse_self_id(&mountinfo));
        Ok(Self { docker, self_id })
    }
}

impl Runtime for DockerRuntime {
    fn name(&self) -> &'static str {
        STREAM_TYPE
    }

    fn self_id(&self) -> Option<String> {
        self.self_id.clone()
    }

    fn watch(&self) -> impl Stream<Item = Result<RuntimeEvent, String>> + Send + 'static {
        let since = SystemTime::now()
            .checked_sub(EVENTS_REPLAY_MARGIN)
            .unwrap_or(UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        let filters = HashMap::from([
            ("type", vec!["container"]),
            ("event", vec!["start", "die", "destroy", "rename"]),
        ]);
        let events = self.docker.events(Some(
            EventsOptionsBuilder::new()
                .since(&since)
                .filters(&filters)
                .build(),
        ));

        let docker = self.docker.clone();
        let snapshot = stream::once(async move {
            let options = ListContainersOptionsBuilder::new().all(false).build();
            docker
                .list_containers(Some(options))
                .await
                .map(|list| {
                    RuntimeEvent::Snapshot(list.into_iter().filter_map(summary_meta).collect())
                })
                .map_err(|err| format!("listing containers: {err}"))
        });

        let docker = self.docker.clone();
        let events = events
            .then(move |event| {
                let docker = docker.clone();
                async move {
                    match event {
                        Ok(event) => event_to_runtime(&docker, event).await,
                        Err(err) => Some(Err(format!("reading events: {err}"))),
                    }
                }
            })
            .filter_map(|event| async move { event });

        snapshot.chain(events)
    }

    fn logs(
        &self,
        id: &str,
        request: LogRequest,
    ) -> impl Stream<Item = Result<LogLine, String>> + Send + 'static {
        let since = request
            .since
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .min(i32::MAX as u64) as i32;
        let options = LogsOptionsBuilder::new()
            .follow(true)
            .stdout(request.stdout)
            .stderr(request.stderr)
            .timestamps(true)
            .since(since)
            .build();
        let raw = Box::pin(self.docker.logs(id, Some(options)));
        let decoder = LogDecoder::new(request.max_line_bytes);

        stream::unfold(
            (raw, decoder, VecDeque::new(), false),
            |(mut raw, mut decoder, mut pending, mut done)| async move {
                loop {
                    if let Some(item) = pending.pop_front() {
                        return Some((item, (raw, decoder, pending, done)));
                    }
                    if done {
                        return None;
                    }
                    match raw.next().await {
                        Some(Ok(output)) => decoder.feed(output, &mut pending),
                        Some(Err(err)) => {
                            decoder.flush(&mut pending);
                            pending.push_back(Err(format!("{err}")));
                            done = true;
                        }
                        None => {
                            decoder.flush(&mut pending);
                            done = true;
                        }
                    }
                }
            },
        )
    }

    fn is_running(&self, id: &str) -> impl Future<Output = Result<bool, String>> + Send + 'static {
        let docker = self.docker.clone();
        let id = id.to_string();
        async move {
            match docker.inspect_container(&id, None).await {
                Ok(inspect) => Ok(inspect
                    .state
                    .and_then(|state| state.running)
                    .unwrap_or(false)),
                Err(err) if is_not_found(&err) => Ok(false),
                Err(err) => Err(format!("{err}")),
            }
        }
    }
}

fn is_not_found(err: &DockerError) -> bool {
    matches!(
        err,
        DockerError::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

fn trim_name(name: &str) -> String {
    name.strip_prefix('/').unwrap_or(name).to_string()
}

fn summary_meta(summary: ContainerSummary) -> Option<ContainerMeta> {
    Some(ContainerMeta {
        id: summary.id?,
        name: summary
            .names
            .and_then(|names| names.into_iter().next())
            .map(|name| trim_name(&name))
            .unwrap_or_default(),
        image: summary.image.unwrap_or_default(),
        labels: summary.labels.unwrap_or_default().into_iter().collect(),
        extra: Default::default(),
    })
}

/// `None` when the container is not running (anymore).
fn inspect_meta(inspect: ContainerInspectResponse) -> Option<ContainerMeta> {
    let running = inspect
        .state
        .as_ref()
        .and_then(|state| state.running)
        .unwrap_or(false);
    if !running {
        return None;
    }
    let config = inspect.config.unwrap_or_default();
    Some(ContainerMeta {
        id: inspect.id?,
        name: inspect.name.as_deref().map(trim_name).unwrap_or_default(),
        image: config.image.unwrap_or_default(),
        labels: config.labels.unwrap_or_default().into_iter().collect(),
        extra: Default::default(),
    })
}

async fn event_to_runtime(
    docker: &Docker,
    event: EventMessage,
) -> Option<Result<RuntimeEvent, String>> {
    let id = event.actor.and_then(|actor| actor.id)?;
    match event.action.as_deref()? {
        "start" | "rename" => match docker.inspect_container(&id, None).await {
            Ok(inspect) => inspect_meta(inspect).map(|meta| Ok(RuntimeEvent::Up(meta))),
            Err(err) if is_not_found(&err) => None,
            // Fail the watch: the supervisor reconnects and resyncs from a snapshot.
            Err(err) => Some(Err(format!("inspecting container {id}: {err}"))),
        },
        "die" => Some(Ok(RuntimeEvent::Down { id, removed: false })),
        "destroy" => Some(Ok(RuntimeEvent::Down { id, removed: true })),
        _ => None,
    }
}

/// Split Docker's `timestamps=1` prefix (`2006-01-02T15:04:05.999999999Z `)
/// from a chunk. Returns the chunk unchanged when there is no valid prefix.
pub fn split_timestamp(chunk: &[u8]) -> (Option<SystemTime>, &[u8]) {
    // RFC 3339 with nanoseconds and a numeric offset is at most 35 bytes.
    let head = &chunk[..chunk.len().min(40)];
    if let Some(space) = head.iter().position(|b| *b == b' ')
        && let Ok(text) = std::str::from_utf8(&head[..space])
        && let Ok(ts) = chrono::DateTime::parse_from_rfc3339(text)
    {
        return (Some(SystemTime::from(ts)), &chunk[space + 1..]);
    }
    (None, chunk)
}

/// Per-connection state turning bollard chunks into lines.
struct LogDecoder {
    stdout: LineAssembler,
    stderr: LineAssembler,
}

impl LogDecoder {
    fn new(max_line_bytes: usize) -> Self {
        Self {
            stdout: LineAssembler::new(max_line_bytes),
            stderr: LineAssembler::new(max_line_bytes),
        }
    }

    fn feed(&mut self, output: LogOutput, pending: &mut VecDeque<Result<LogLine, String>>) {
        let (source, message) = match output {
            // TTY containers have a single merged output.
            LogOutput::StdOut { message } | LogOutput::Console { message } => {
                (Source::Stdout, message)
            }
            LogOutput::StdErr { message } => (Source::Stderr, message),
            LogOutput::StdIn { .. } => return,
        };
        let (ts, data) = split_timestamp(&message);
        let assembler = match source {
            Source::Stdout => &mut self.stdout,
            Source::Stderr => &mut self.stderr,
        };
        let mut lines = Vec::new();
        assembler.push(data, ts, &mut lines);
        pending.extend(
            lines
                .into_iter()
                .map(|(data, ts)| Ok(LogLine { ts, source, data })),
        );
    }

    fn flush(&mut self, pending: &mut VecDeque<Result<LogLine, String>>) {
        for (source, assembler) in [
            (Source::Stdout, &mut self.stdout),
            (Source::Stderr, &mut self.stderr),
        ] {
            if let Some((data, ts)) = assembler.flush() {
                pending.push_back(Ok(LogLine { ts, source, data }));
            }
        }
    }
}

/// Find the ID of the container we run in from `/proc/self/mountinfo`.
///
/// Docker bind-mounts `/var/lib/docker/containers/<id>/hostname` on
/// `/etc/hostname` in every container. Only that mount point is considered, so
/// running on a host with containers (whose own mounts appear in the host's
/// mountinfo) never yields a false positive.
pub fn parse_self_id(mountinfo: &str) -> Option<String> {
    mountinfo.lines().find_map(|line| {
        let mut fields = line.split(' ');
        let root = fields.nth(3)?;
        let mount_point = fields.next()?;
        if mount_point != "/etc/hostname" {
            return None;
        }
        let rest = &root[root.find("/containers/")? + "/containers/".len()..];
        let id = rest.strip_suffix("/hostname")?;
        (id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id.to_string())
    })
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn timestamps() {
        let (ts, rest) = split_timestamp(b"2026-11-22T10:00:01.123456789Z hello world\n");
        assert_eq!(rest, b"hello world\n");
        let ts = ts.unwrap().duration_since(UNIX_EPOCH).unwrap();
        assert_eq!(ts.subsec_nanos(), 123_456_789);

        let (ts, rest) = split_timestamp(b"2026-11-22T10:00:01+02:00 x");
        assert!(ts.is_some());
        assert_eq!(rest, b"x");

        let (ts, rest) = split_timestamp(b"no timestamp here");
        assert!(ts.is_none());
        assert_eq!(rest, b"no timestamp here");

        let (ts, rest) = split_timestamp(b"");
        assert!(ts.is_none());
        assert_eq!(rest, b"");
    }

    #[test]
    fn decoder_joins_partial_chunks() {
        let mut decoder = LogDecoder::new(1024);
        let mut pending = VecDeque::new();
        let out = |s: &'static str| LogOutput::StdOut {
            message: Bytes::from_static(s.as_bytes()),
        };
        decoder.feed(out("2026-11-22T10:00:01Z part1-"), &mut pending);
        decoder.feed(out("2026-11-22T10:00:02Z part2\n"), &mut pending);
        decoder.feed(
            LogOutput::StdErr {
                message: Bytes::from_static(b"2026-11-22T10:00:03Z err\n"),
            },
            &mut pending,
        );
        decoder.feed(
            LogOutput::Console {
                message: Bytes::from_static(b"2026-11-22T10:00:04Z tty\r\n"),
            },
            &mut pending,
        );
        decoder.feed(out("2026-11-22T10:00:05Z tail"), &mut pending);
        decoder.flush(&mut pending);

        let lines: Vec<_> = pending
            .into_iter()
            .map(|l| {
                let l = l.unwrap();
                (l.source, String::from_utf8(l.data).unwrap())
            })
            .collect();
        assert_eq!(
            lines,
            vec![
                (Source::Stdout, "part1-part2".into()),
                (Source::Stderr, "err".into()),
                (Source::Stdout, "tty\r".into()),
                (Source::Stdout, "tail".into()),
            ]
        );
    }

    #[test]
    fn self_id() {
        let id = "a".repeat(64);
        let inside = format!(
            "1 0 0:1 / / rw - overlay overlay rw\n\
             2 1 254:1 /docker/containers/{id}/resolv.conf /etc/resolv.conf rw - ext4 /dev/vda1 rw\n\
             3 1 254:1 /docker/containers/{id}/hostname /etc/hostname rw - ext4 /dev/vda1 rw\n"
        );
        assert_eq!(parse_self_id(&inside), Some(id.clone()));

        let host = format!(
            "1 0 0:1 / / rw - ext4 /dev/vda1 rw\n\
             2 1 0:2 / /var/lib/docker/containers/{id}/mounts/shm rw - tmpfs shm rw\n"
        );
        assert_eq!(parse_self_id(&host), None);
        assert_eq!(parse_self_id(""), None);
    }

    #[test]
    fn host_validation() {
        for ok in [
            None,
            Some("unix:///run/docker.sock"),
            Some("tcp://127.0.0.1:2375"),
        ] {
            let options = DockerOptions {
                host: ok.map(String::from),
            };
            assert!(options.validate().is_ok());
        }
        let options = DockerOptions {
            host: Some("ssh://host".into()),
        };
        assert!(options.validate().is_err());
    }
}
