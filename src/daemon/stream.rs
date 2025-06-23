use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    process::Stdio,
    task::Poll,
    time::Duration,
};

use chrono::Local;
use futures::{FutureExt, StreamExt};
use regex::RegexSet;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    pin,
    process::{Child, ChildStderr, ChildStdout, Command},
    time::sleep,
};
use tracing::{error, info, warn};

use crate::{
    concepts::{Filter, Stream},
    daemon::filter::FilterManager,
};

use super::shutdown::ShutdownToken;

/** Converts bytes to string, discarding invalid utf8 sequences
*/
fn to_string(data: &[u8]) -> String {
    let res = String::from_utf8_lossy(data);
    res.to_string()
        .replace(std::char::REPLACEMENT_CHARACTER, "")
}

#[allow(clippy::type_complexity)]
fn lines_to_stream<T: tokio::io::AsyncRead + Unpin>(
    mut lines: BufReader<T>,
) -> futures::stream::PollFn<
    impl FnMut(&mut std::task::Context) -> Poll<Option<Result<String, std::io::Error>>>,
> {
    let mut at_eof = false;
    let mut buffer = vec![];
    futures::stream::poll_fn(move |cx| {
        if at_eof {
            // reached EOF earlier, avoid calling read again
            return Poll::Ready(None);
        }
        // Try to read until LF or EOF. If interrupted, buffer might contain data, in which case
        // new data will be happened to it
        let nl = lines.read_until(0x0a, &mut buffer);
        pin!(nl);
        match futures::Future::poll(nl, cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(0)) => {
                if buffer.is_empty() {
                    // at eof
                    Poll::Ready(None)
                } else {
                    // reached eof with data in the buffer
                    at_eof = true;
                    let line = to_string(&buffer);
                    buffer.clear();
                    Poll::Ready(Some(Ok(line)))
                }
            }
            Poll::Ready(Ok(_)) => {
                let line = to_string(&buffer);
                buffer.clear();
                Poll::Ready(Some(Ok(line)))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
        }
    })
}

pub struct StreamManager {
    compiled_regex_set: RegexSet,
    regex_index_to_filter_manager: Vec<FilterManager>,
    stream: &'static Stream,
    shutdown: ShutdownToken,
}

impl StreamManager {
    pub fn new(
        stream: &'static Stream,
        filter_managers: HashMap<&'static Filter, FilterManager>,
        shutdown: ShutdownToken,
    ) -> Result<Self, regex::Error> {
        let all_regexes: BTreeMap<_, _> = filter_managers
            .iter()
            .flat_map(|(filter, filter_manager)| {
                filter
                    .regex()
                    .iter()
                    .map(|regex| (regex, filter_manager.clone()))
            })
            .collect();

        Ok(StreamManager {
            compiled_regex_set: RegexSet::new(all_regexes.keys())?,
            regex_index_to_filter_manager: all_regexes.into_values().collect(),
            stream,
            shutdown,
        })
    }

    pub async fn start(self) {
        info!("{}: start {:?}", self.stream.name(), self.stream.cmd());
        let mut child = match Command::new(&self.stream.cmd()[0])
            .args(&self.stream.cmd()[1..])
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                error!(
                    "could not execute stream {} cmd: {}",
                    self.stream.name(),
                    err
                );
                return;
            }
        };

        // keep stdout/stderr before moving child to handle_child
        #[allow(clippy::unwrap_used)]
        // we know there is an stdout because we asked for Stdio::piped()
        let child_stdout = child.stdout.take().unwrap();
        #[allow(clippy::unwrap_used)]
        // we know there is an stderr because we asked for Stdio::piped()
        let child_stderr = child.stderr.take().unwrap();

        tokio::join!(
            self.handle_child(child),
            self.handle_io(child_stdout, child_stderr),
        );
    }

    async fn handle_child(&self, mut child: Child) {
        const STREAM_PROCESS_GRACE_TIME_SEC: u64 = 15;
        const STREAM_PROCESS_KILL_WAIT_TIMEOUT_SEC: u64 = 5;

        // wait either for the child process to exit on its own or for the shutdown signal
        futures::select! {
            _ = child.wait().fuse() => {
                error!("stream {} exited: its command returned.", self.stream.name());
                return;
            }
            _ = self.shutdown.wait().fuse() => {}
        }

        // first, try to ask nicely the child process to exit
        if let Some(pid) = child.id() {
            let pid = nix::unistd::Pid::from_raw(pid as i32);

            // the most likely error is that the process does not exist anymore
            // but we still need to reclaim it with Child::wait
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::SIGTERM);

            futures::select! {
                _ = child.wait().fuse() => {
                    return;
                },
                _ = sleep(Duration::from_secs(STREAM_PROCESS_GRACE_TIME_SEC)).fuse() => {},
            }
        } else {
            warn!(
                "could not get PID of child process for stream {}",
                self.stream.name()
            );
            // still try to use tokio API to kill and reclaim the child process
        }

        // if that fails, or we cannot get the underlying PID, terminate the process.
        // NOTE: processes killed with SIGKILL are not guaranteed to exit. They can be locked up in a
        // syscall to a resource no-longer available (a notorious example is a read on a disconnected
        // NFS share)

        // as before, the only expected error is that the child process already terminated
        // but we still need to reclaim it if that's the case.
        let _ = child.start_kill();

        futures::select! {
            _ = child.wait().fuse() => {}
            _ = sleep(Duration::from_secs(STREAM_PROCESS_KILL_WAIT_TIMEOUT_SEC)).fuse() => {
                error!("child process of stream {} did not terminate", self.stream.name());
            }
        }
    }

    async fn handle_io(&self, child_stdout: ChildStdout, child_stderr: ChildStderr) {
        let lines_stdout = lines_to_stream(BufReader::new(child_stdout));
        let lines_stderr = lines_to_stream(BufReader::new(child_stderr));
        // aggregate outputs, will end when both streams end
        let mut lines = futures::stream::select(lines_stdout, lines_stderr);

        loop {
            match lines.next().await {
                Some(Ok(line)) => {
                    let now = Local::now();
                    for manager in self.matching_filters(&line) {
                        manager.handle_line(&line, now);
                    }
                }
                Some(Err(err)) => {
                    error!(
                        "impossible to read output from stream {}: {}",
                        self.stream.name(),
                        err
                    );
                    return;
                }
                None => {
                    return;
                }
            }
        }
    }

    fn matching_filters(&self, line: &str) -> BTreeSet<&FilterManager> {
        let matches = self.compiled_regex_set.matches(line);
        matches
            .into_iter()
            .map(|match_| &self.regex_index_to_filter_manager[match_])
            .collect()
    }
}
