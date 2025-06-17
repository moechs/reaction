use std::{collections::HashMap, process::Stdio, task::Poll, time::Duration};

use chrono::Local;
use futures::{FutureExt, StreamExt};
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
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

#[allow(clippy::type_complexity)]
fn lines_to_stream<T: tokio::io::AsyncBufRead + Unpin>(
    mut lines: Lines<T>,
) -> futures::stream::PollFn<
    impl FnMut(&mut std::task::Context) -> Poll<Option<Result<String, std::io::Error>>>,
> {
    futures::stream::poll_fn(move |cx| {
        let nl = lines.next_line();
        pin!(nl);
        futures::Future::poll(nl, cx).map(Result::transpose)
    })
}

pub async fn stream_manager(
    stream: &'static Stream,
    filter_managers: HashMap<&'static Filter, FilterManager>,
    shutdown: ShutdownToken,
) {
    info!("{}: start {:?}", stream.name(), stream.cmd());
    let mut child = match Command::new(&stream.cmd()[0])
        .args(&stream.cmd()[1..])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            error!("could not execute stream {} cmd: {}", stream.name(), err);
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
        handle_child(stream.name(), child, shutdown),
        handle_io(stream.name(), child_stdout, child_stderr, filter_managers)
    );
}

async fn handle_child(stream_name: &'static str, mut child: Child, shutdown: ShutdownToken) {
    const STREAM_PROCESS_GRACE_TIME_SEC: u64 = 15;
    const STREAM_PROCESS_KILL_WAIT_TIMEOUT_SEC: u64 = 5;

    // wait either for the child process to exit on its own or for the shutdown signal
    futures::select! {
        _ = child.wait().fuse() => {
            error!("stream {stream_name} exited: its command returned.");
            return;
        }
        _ = shutdown.wait().fuse() => {}
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
        warn!("could not get PID of child process for stream {stream_name}");
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
            error!("child process of stream {stream_name} did not terminate");
        }
    }
}

async fn handle_io(
    stream_name: &'static str,
    child_stdout: ChildStdout,
    child_stderr: ChildStderr,
    filter_managers: HashMap<&'static Filter, FilterManager>,
) {
    let lines_stdout = lines_to_stream(BufReader::new(child_stdout).lines());
    let lines_stderr = lines_to_stream(BufReader::new(child_stderr).lines());
    // aggregate outputs, will end when both streams end
    let mut lines = futures::stream::select(lines_stdout, lines_stderr);

    loop {
        match lines.next().await {
            Some(Ok(line)) => {
                let now = Local::now();
                for manager in filter_managers.values() {
                    manager.handle_line(&line, now);
                }
            }
            Some(Err(err)) => {
                error!(
                    "impossible to read output from stream {}: {}",
                    stream_name, err
                );
                return;
            }
            None => {
                return;
            }
        }
    }
}
