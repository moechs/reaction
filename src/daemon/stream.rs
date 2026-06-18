use std::{
    collections::{BTreeSet, HashMap},
    process::Stdio,
};

use futures::{FutureExt, Stream as AsyncStream, StreamExt, future::join_all};
use reaction_plugin::{StreamImpl, shutdown::ShutdownToken, time::print_duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, ChildStdout, Command},
    time::sleep,
};
use tracing::{debug, error, info};

use crate::{
    concepts::{Behavior, Filter, FinallyBehavior, LineContext, Stream, Time, now},
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
    regex_index_to_filter_manager: Vec<FilterManager>,
    no_regex_filter_managers: Vec<FilterManager>,
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
        let stream_plugin = if stream.is_plugin() {
            Some(
                plugins
                    .get_stream_impl(stream.name.clone())
                    .ok_or_else(|| {
                        format!(
                            "stream {} doesn't load a plugin. this is a bug!",
                            stream.name
                        )
                    })?,
            )
        } else {
            None
        };

        let find_filter = |filter_name| {
            filter_managers
                .iter()
                .find(|(filter, _)| filter_name == &filter.name)
                .unwrap()
                .1
                .clone()
        };

        let regex_index_to_filter_manager = stream
            .regex_index_to_filter_name
            .iter()
            .map(find_filter)
            .collect();

        let no_regex_filter_managers = stream
            .filters
            .iter()
            .filter(|(_, filter)| filter.hint_regexes().is_none())
            .map(|(_, filter)| &filter.name)
            .map(find_filter)
            .collect();

        debug!("successfully initialized stream {}", stream.name);

        Ok(StreamManager {
            regex_index_to_filter_manager,
            no_regex_filter_managers,
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
        let now = now();

        let unique_filter_managers: BTreeSet<_> = self
            .regex_index_to_filter_manager
            .iter()
            .chain(self.no_regex_filter_managers.iter())
            .collect();

        join_all(
            unique_filter_managers
                .into_iter()
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
                    self.handle_line(line, time.into()).await;
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
        self.start_cmd_no_restart().await;

        let eof = &self.stream.eof;
        match eof.behavior {
            Behavior::Ignore => {}
            Behavior::Exit => self.shutdown.ask_shutdown(),
            Behavior::Restart => {
                let mut retries = 0;
                while eof.retry == 0 || retries < eof.retry {
                    retries += 1;

                    info!(
                        "{}: cmd exited, sleeping {} before retry n°{retries}",
                        self.stream.name,
                        print_duration(eof.after.as_ref().unwrap()),
                    );
                    sleep(eof.after.unwrap()).await;

                    self.start_cmd_no_restart().await;
                }

                if eof.finally == Some(FinallyBehavior::Exit) {
                    self.shutdown.ask_shutdown();
                }
            }
        }
    }

    async fn start_cmd_no_restart(&self) {
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
                    let now = now();
                    self.handle_line(line, now).await;
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
        let mut context = LineContext::default();
        for manager in self.matching_filters(&line) {
            manager.handle_line(&line, time, &mut context).await;
        }
    }

    fn matching_filters(&self, line: &str) -> BTreeSet<&FilterManager> {
        let matches = self.stream.compiled_regex_set.matches(line);
        matches
            .into_iter()
            .map(|match_| &self.regex_index_to_filter_manager[match_])
            .chain(self.no_regex_filter_managers.iter())
            .collect()
    }
}
