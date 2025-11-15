use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    process::Stdio,
};

use chrono::Local;
use futures::{FutureExt, Stream as AsyncStream, StreamExt, future::join_all};
use reaction_plugin::{StreamImpl, shutdown::ShutdownToken};
use regex::RegexSet;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, ChildStdout, Command},
};
use tracing::{debug, error, info};

use crate::{
    concepts::{Filter, Stream, Time},
    daemon::{filter::FilterManager, plugin::Plugins, utils::kill_child},
};

/// Converts bytes to line string, discarding invalid utf8 sequences and newlines at the end
fn to_line(data: &[u8]) -> String {
    String::from_utf8_lossy(data)
        .trim_end_matches('\n')
        .replace(std::char::REPLACEMENT_CHARACTER, "")
}

pub fn reader_to_stream(
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
    pub async fn new(
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
            Some(
                plugins
                    .init_stream_impl(
                        stream.name.clone(),
                        stream.stream_type.clone().unwrap(),
                        stream.options.clone(),
                    )
                    .await?,
            )
        } else {
            None
        };

        debug!("successfully initialized stream {}", stream.name);

        Ok(StreamManager {
            compiled_regex_set: RegexSet::new(all_regexes.keys()).map_err(|err| err.to_string())?,
            regex_index_to_filter_manager: all_regexes.into_values().collect(),
            stream,
            stream_plugin,
            shutdown,
        })
    }

    pub fn is_standalone(&self) -> bool {
        match &self.stream_plugin {
            Some(plugin) => plugin.standalone,
            None => true,
        }
    }

    pub async fn start(mut self) {
        // First start FilterManagers persisted actions
        let now = Local::now();
        join_all(
            self.regex_index_to_filter_manager
                .iter()
                .map(|filter_manager| filter_manager.start(now)),
        )
        .await;

        // Then start stream
        info!("{}: start {:?}", self.stream.name, self.stream.cmd);

        if self.stream_plugin.is_some() {
            self.start_plugin().await
        } else {
            self.start_cmd().await
        }
    }

    async fn start_plugin(&mut self) {
        let mut plugin = self.stream_plugin.take().unwrap();

        loop {
            match plugin.stream.recv().await {
                Ok(Some((line, time))) => {
                    self.handle_line(line, time).await;
                }
                Err(err) => {
                    if err.is_final() {
                        error!(
                            "error reading from plugin stream {}: {}",
                            self.stream.name, err
                        );
                        return;
                    } else {
                        error!(
                            "temporary error reading from plugin stream {}: {}",
                            self.stream.name, err
                        );
                    }
                }
                Ok(None) => {
                    if !self.shutdown.is_shutdown() {
                        error!("stream {} has exited", self.stream.name);
                    }
                    return;
                }
            }
        }
    }

    async fn start_cmd(&self) {
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
        // wait either for the child process to exit on its own or for the shutdown signal
        futures::select! {
            _ = child.wait().fuse() => {
                error!("stream {} exited: its command returned.", self.stream.name);
                return;
            }
            _ = self.shutdown.wait().fuse() => {}
        }

        kill_child(child, format!("stream {}", self.stream.name), 15).await;
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
                    self.handle_line(line, Local::now()).await;
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

    async fn handle_line(&self, line: String, time: Time) {
        for manager in self.matching_filters(&line) {
            manager.handle_line(&line, time).await;
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
