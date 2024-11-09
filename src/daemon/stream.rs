use std::{collections::HashMap, process::Stdio, sync::Arc, task::Poll};

use futures::StreamExt;
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    pin,
    process::{Child, Command},
    sync::{oneshot, Mutex},
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
    child_tx: oneshot::Sender<Option<Child>>,
    filter_managers: Arc<Mutex<HashMap<&'static Filter, FilterManager>>>,
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
            let _ = child_tx.send(None);
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

    // let main handle the child process
    let _ = child_tx.send(Some(child));

    loop {
        match lines.next().await {
            Some(Ok(line)) => {
                futures::future::join_all(
                    filter_managers
                        .lock()
                        .await
                        .values_mut()
                        .map(|manager| manager.handle_line(&line)),
                )
                .await;
            }
            Some(Err(err)) => {
                error!(
                    "impossible to read output from stream {}: {}",
                    stream.name(),
                    err
                );
                break;
            }
            None => {
                error!("stream {} exited: its command returned.", stream.name());
                break;
            }
        }
    }

    filter_managers
        .lock()
        .await
        .values_mut()
        .for_each(|manager| manager.quit());
}
