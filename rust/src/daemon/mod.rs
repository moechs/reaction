use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use log::{error, info};
use socket::socket_manager;
use tokio::{
    join,
    process::Child,
    select,
    signal::unix::{signal, SignalKind},
    sync::{mpsc, oneshot},
};

use crate::concepts::Config;
use database::database_manager;
use execs::execs_manager;
use matches::matches_manager;

pub use execs::ExecsManagerInput;
pub use matches::MatchManagerInput;

mod database;
mod execs;
mod matches;
mod socket;
mod statemap;

pub async fn daemon(config_path: PathBuf, socket: PathBuf) -> Result<(), Box<dyn Error>> {
    let config: &'static Config =
        Config::from_file(&config_path).map(|config| Box::leak(Box::new(config)))?;

    if !config.start() {
        return Err("a start command failed, exiting.".into());
    }

    let mut stream_process_child_handles = Vec::new();
    let mut stream_task_handles = Vec::new();

    let (match_tx, match_rx) = mpsc::channel(10);
    let (exec_tx, exec_rx) = mpsc::channel(10);
    let (log_tx, log_rx) = mpsc::channel(10);

    // TODO use this for a more idiomatic shutdown signal
    // let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let matches_manager_task_handle = {
        let match_tx_matches = match_tx.clone();
        let exec_tx_matches = exec_tx.clone();
        let log_tx_matches = log_tx.clone();
        tokio::spawn(async move {
            matches_manager(match_rx, match_tx_matches, exec_tx_matches, log_tx_matches).await
        })
    };

    let execs_manager_task_handle = {
        let exec_tx_execs = exec_tx.clone();
        tokio::spawn(async move { execs_manager(config, exec_rx, exec_tx_execs, log_tx).await })
    };

    let database_manager_task_handle = {
        let match_tx_database = match_tx.clone();
        // The `task::spawn` is done in the function, after database rotation is finished
        database_manager(config, log_rx, match_tx_database).await
    };

    {
        let match_tx_socket = match_tx.clone();
        let exec_tx_socket = exec_tx.clone();
        let socket = socket.to_owned();
        tokio::spawn(async move {
            socket_manager(config, socket, match_tx_socket, exec_tx_socket).await
        });
    }

    for stream in config.streams().values() {
        let match_tx = match_tx.clone();
        let (child_tx, child_rx) = oneshot::channel();

        stream_task_handles.push(tokio::spawn(async move {
            stream.manager(child_tx, match_tx).await
        }));

        if let Ok(Some(child)) = child_rx.await {
            stream_process_child_handles.push(child);
        }
    }

    // TODO replace by async things
    tokio::spawn(async move {});
    let signal_received = Arc::new(AtomicBool::new(false));
    {
        // Handle SIGINT, SIGTERM, SIGHUP
        let signal_received2 = signal_received.clone();
        if let Err(err) = ctrlc::set_handler(move || {
            signal_received2.store(true, Ordering::SeqCst);

            info!("waiting for streams to finish...");
            // Kill stream subprocesses
            for child_handle in stream_process_child_handles.into_iter() {
                let _ = tokio::spawn(async move { child_handle.kill().await });
            }
        }) {
            match err {
                // Would only happen in tests as this is the only place in the code
                // where we put an handler. Sorry that there is test-specific code!
                ctrlc::Error::MultipleHandlers => {}
                // Real error
                _ => {
                    return Err(
                        "impossible to launch a signal-catching thread, exiting: {err}".into(),
                    );
                }
            }
        }
    }

    // Wait for all streams to quit
    for task_handle in stream_task_handles {
        let _ = task_handle.await;
    }

    #[allow(clippy::unwrap_used)] // propagating panics is ok
    match_tx.send(MatchManagerInput::Stop).await.unwrap();
    let _ = matches_manager_task_handle.await;

    #[allow(clippy::unwrap_used)] // propagating panics is ok
    exec_tx.send(ExecsManagerInput::Stop).await.unwrap();
    let _ = execs_manager_task_handle.await;

    let _ = database_manager_task_handle.await;

    let stop_ok = config.stop();

    // not waiting for the socket_manager to finish, sorry
    // TODO make it listen on shutdown_rx
    if let Err(err) = fs::remove_file(socket) {
        error!("failed to remove socket: {}", err);
    }

    if !signal_received.load(Ordering::SeqCst) {
        Err("quitting because all streams finished".into())
    } else if !stop_ok {
        Err("while executing stop command".into())
    } else {
        Ok(())
    }
}

async fn signals(stream_process_child_handles: Vec<Child>) -> tokio::io::Result<()> {
    // TODO refaire pour que la création des channels soient sync puis que la suite soit spawn
    {
        let mut sighup = signal(SignalKind::hangup())?;
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        select! {
            _ = sighup.recv() => {},
            _ = sigint.recv() => {},
            _ = sigterm.recv() => {},
        };
    }
    Ok(())
}
