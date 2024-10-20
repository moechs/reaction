use std::{collections::BTreeMap, process::Stdio, sync::Arc};

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::{oneshot, Mutex},
};
use tracing::{error, info};

use crate::{
    concepts::{Filter, Stream},
    daemon::filter::FilterManager,
};

pub async fn stream_manager(
    stream: &'static Stream,
    child_tx: oneshot::Sender<Option<Child>>,
    filter_managers: Arc<Mutex<BTreeMap<&'static Filter, FilterManager>>>,
) {
    info!("{}: start {:?}", stream.name(), stream.cmd());
    let mut child = match Command::new(&stream.cmd()[0])
        .args(&stream.cmd()[1..])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
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
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();

    // let main handle the child process
    let _ = child_tx.send(Some(child));

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                futures::future::join_all(
                    filter_managers
                        .lock()
                        .await
                        .values_mut()
                        .map(|manager| manager.handle_line(&line)),
                )
                .await;
            }
            Ok(None) => {
                error!("stream {} exited: its command returned.", stream.name());
                break;
            }
            Err(err) => {
                error!(
                    "impossible to read output from stream {}: {}",
                    stream.name(),
                    err
                );
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
