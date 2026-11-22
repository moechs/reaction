//! The contract between the runtime-agnostic core and a backend.

use std::{future::Future, time::SystemTime};

use futures::Stream;

use super::meta::ContainerMeta;

/// Which output of the container a line comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Stdout,
    Stderr,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Stdout => "stdout",
            Source::Stderr => "stderr",
        }
    }
}

/// A complete log line, without its trailing newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// Timestamp reported by the runtime, when it could be parsed.
    pub ts: Option<SystemTime>,
    pub source: Source,
    /// Raw bytes; the core converts them to text.
    pub data: Vec<u8>,
}

/// What the core asks for when opening a log stream.
#[derive(Debug, Clone)]
pub struct LogRequest {
    pub stdout: bool,
    pub stderr: bool,
    /// Lines older than this may be skipped by the runtime. The core still
    /// drops already-seen lines itself, so backends may round it down.
    pub since: SystemTime,
    /// Longer lines are truncated to this many bytes.
    pub max_line_bytes: usize,
}

/// Change notifications produced by [`Runtime::watch`].
#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    /// Every container running right now. Always the first item of a watch,
    /// so the supervisor can reconcile after a reconnection.
    Snapshot(Vec<ContainerMeta>),
    /// A container is running with this metadata (started, restarted, renamed...).
    /// May be sent for a container that is already known: it must be idempotent.
    Up(ContainerMeta),
    /// A container stopped. `removed` is true when it will never come back
    /// (its ID can be forgotten).
    Down { id: String, removed: bool },
}

/// A container runtime backend.
///
/// Errors are plain strings: the core logs them and retries after
/// `reconnect_delay`. Returned futures and streams must be `Send + 'static`
/// because the core spawns them on tokio tasks.
pub trait Runtime: Send + Sync + 'static {
    /// Short name used in log messages, e.g. `docker`.
    fn name(&self) -> &'static str;

    /// ID of the container this plugin runs in, if any and if detectable.
    /// Its logs are never followed, to avoid feedback loops.
    fn self_id(&self) -> Option<String>;

    /// Watch running containers. The stream starts with a
    /// [`RuntimeEvent::Snapshot`] and must not miss any event that happens
    /// after the snapshot was taken. The core drops the stream at its first
    /// error or end and calls `watch` again later.
    fn watch(&self) -> impl Stream<Item = Result<RuntimeEvent, String>> + Send + 'static;

    /// Follow the logs of one container. The stream ends when the container
    /// stops or the connection breaks.
    fn logs(
        &self,
        id: &str,
        request: LogRequest,
    ) -> impl Stream<Item = Result<LogLine, String>> + Send + 'static;

    /// Whether the container is still running. `Ok(false)` when it does not exist.
    fn is_running(&self, id: &str) -> impl Future<Output = Result<bool, String>> + Send + 'static;
}
