use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    process::Stdio,
    time::Duration,
};

use chrono::Local;
use futures::{FutureExt, Stream as AsyncStream, StreamExt};
use reaction_plugin::StreamImpl;
use regex::RegexSet;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, ChildStdout, Command},
    time::sleep,
};
use tracing::{error, info, warn};

use crate::{
    concepts::{Filter, Stream},
    daemon::{filter::FilterManager, plugin::Plugins},
};

use super::shutdown::ShutdownToken;

/// Converts bytes to line string, discarding invalid utf8 sequences and newlines at the end
fn to_line(data: &[u8]) -> String {
    String::from_utf8_lossy(data)
        .trim_end_matches('\n')
        .replace(std::char::REPLACEMENT_CHARACTER, "")
}

fn reader_to_stream(
    reader: impl tokio::io::AsyncRead + Unpin,
) -> impl AsyncStream<Item = Result<String, std::io::Error>> {
    let buf_reader = BufReader::new(reader);
    let buffer = vec![];
    futures::stream::try_unfold(
        (buf_reader, buffer),
        |(mut buf_reader, mut buffer)| async move {
            let nl = buf_reader.read_until(b'\n', &mut buffer).await?;
            if nl > 0 {
                let line = to_line(&buffer);
                buffer.clear();
                Ok(Some((line, (buf_reader, buffer))))
            } else {
                Ok(None)
            }
        },
    )
}

pub struct StreamManager {
    compiled_regex_set: RegexSet,
    regex_index_to_filter_manager: Vec<FilterManager>,
    stream: &'static Stream,
    stream_plugin: Option<StreamImpl>,
    shutdown: ShutdownToken,
}

impl StreamManager {
    pub fn new(
        stream: &'static Stream,
        filter_managers: HashMap<&'static Filter, FilterManager>,
        shutdown: ShutdownToken,
        plugins: &mut Plugins,
    ) -> Result<Self, String> {
        let all_regexes: BTreeMap<_, _> = filter_managers
            .iter()
            .flat_map(|(filter, filter_manager)| {
                filter
                    .regex
                    .iter()
                    .map(|regex| (regex, filter_manager.clone()))
            })
            .collect();

        let stream_plugin = if stream.is_plugin() {
            Some(plugins.init_stream_impl(
                stream.name.clone(),
                stream.stream_type.clone().unwrap(),
                stream.options.clone(),
            )?)
        } else {
            None
        };

        Ok(StreamManager {
            compiled_regex_set: RegexSet::new(all_regexes.keys()).map_err(|err| err.to_string())?,
            regex_index_to_filter_manager: all_regexes.into_values().collect(),
            stream,
            stream_plugin,
            shutdown,
        })
    }

    pub async fn start(self) {
        info!("{}: start {:?}", self.stream.name, self.stream.cmd);

        if self.stream_plugin.is_some() {
            self.start_plugin().await
        } else {
            self.start_cmd().await
        }
    }

    async fn start_plugin(self) {
        let plugin = self.stream_plugin.unwrap();

        {
            let result = plugin.start().await;
            if result.is_err() {
                error!(
                    "could not execute stream {}: {}",
                    self.stream.name,
                    result.unwrap_err()
                );
                return;
            }
        }

        loop {
            let result = plugin.next().await;
            let result = if result.is_ok() {
                let option = result.unwrap();
                if option.is_some() {
                    self.handle_line(option.unwrap().to_string()).await;
                } else {
                    return;
                }
            } else {
                error!(
                    "impossible to read output from stream {}: {}",
                    self.stream.name,
                    result.unwrap_err()
                );
                return;
            };
        }
    }

    async fn start_cmd(self) {
        let mut child = match Command::new(&self.stream.cmd[0])
            .args(&self.stream.cmd[1..])
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                error!("could not execute stream {} cmd: {}", self.stream.name, err);
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
                error!("stream {} exited: its command returned.", self.stream.name);
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
                self.stream.name
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
                error!("child process of stream {} did not terminate", self.stream.name);
            }
        }
    }

    async fn handle_io(&self, child_stdout: ChildStdout, child_stderr: ChildStderr) {
        let lines_stdout = reader_to_stream(child_stdout);
        let lines_stderr = reader_to_stream(child_stderr);
        // aggregate outputs, will end when both streams end
        let lines = futures::stream::select(lines_stdout, lines_stderr);
        tokio::pin!(lines);

        loop {
            match lines.next().await {
                Some(Ok(line)) => {
                    self.handle_line(line).await;
                }
                Some(Err(err)) => {
                    error!(
                        "impossible to read output from stream {}: {}",
                        self.stream.name, err
                    );
                    return;
                }
                None => {
                    return;
                }
            }
        }
    }

    async fn handle_line(&self, line: String) {
        let now = Local::now();
        for manager in self.matching_filters(&line) {
            manager.handle_line(&line, now).await;
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
