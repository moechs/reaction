use std::{collections::HashMap, process::Stdio, sync::Arc, task::Poll};

use futures::{pin_mut, FutureExt, StreamExt};
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    pin,
    process::Command,
    sync::{broadcast, Mutex},
};
use tracing::{error, info};

use crate::{
    concepts::{Filter, Stream},
    daemon::filter::FilterManager,
};

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
    filter_managers: Arc<Mutex<HashMap<&'static Filter, FilterManager>>>,
    mut shutdown_rx: broadcast::Receiver<()>,
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

    // keep stdout before sending/moving child to the main thread
    #[allow(clippy::unwrap_used)]
    // we know there is an stdout because we asked for Stdio::piped()
    let lines_stdout = lines_to_stream(BufReader::new(child.stdout.take().unwrap()).lines());
    #[allow(clippy::unwrap_used)]
    // we know there is an stderr because we asked for Stdio::piped()
    let lines_stderr = lines_to_stream(BufReader::new(child.stderr.take().unwrap()).lines());

    // aggregate outputs, will end when both streams end
    let mut lines = futures::stream::select(lines_stdout, lines_stderr);

    // fuse the future right after the first and only message we expect on this channel
    let shutdown_watch = shutdown_rx.recv().fuse();
    pin_mut!(shutdown_watch);

    let mut child_killed = false;

    loop {
        futures::select!(
            _ = shutdown_watch => {
                // reaction is shutting down, kill child process, we'll reap it after
                // processing the remaining I/O
                let _ = child.start_kill();
                child_killed = true;
            },
            line = lines.next() => {
                if handle_io(line, &filter_managers, stream.name(), child_killed).await.is_err() {
                    break;
                }
            }
        );
    }

    // reap the child process if already dead or kill it (might issue sigkill twice with the one
    // above).
    match child.try_wait() {
        Ok(Some(_)) => {}
        _ => {
            let _ = child.kill().await;
        }
    }

    filter_managers
        .lock()
        .await
        .values_mut()
        .for_each(|manager| manager.quit());
}

async fn handle_io(
    line: Option<Result<String, std::io::Error>>,
    filter_managers: &Arc<Mutex<HashMap<&'static Filter, FilterManager>>>,
    stream_name: &str,
    child_killed: bool,
) -> Result<(), ()> {
    match line {
        Some(Ok(line)) => {
            futures::future::join_all(
                filter_managers
                    .lock()
                    .await
                    .values_mut()
                    .map(|manager| manager.handle_line(&line)),
            )
            .await;
            Ok(())
        }
        Some(Err(err)) => {
            if !child_killed {
                error!(
                    "impossible to read output from stream {}: {}",
                    stream_name, err
                );
            }
            Err(())
        }
        None => {
            if !child_killed {
                error!("stream {} exited: its command returned.", stream_name);
            }
            Err(())
        }
    }
}
