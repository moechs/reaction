use std::{error::Error, fs, path::PathBuf};

use socket::socket_manager;
use tokio::{
    process::Child,
    select,
    signal::unix::{signal, SignalKind},
    sync::{mpsc, oneshot, watch},
};
use tracing::{error, info};

use crate::concepts::Config;
use database::database_manager;
use execs::execs_manager;
use matches::matches_manager;

mod database;
mod execs;
mod matches;
mod socket;
mod statemap;

pub async fn daemon(
    config_path: PathBuf,
    socket: PathBuf,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config: &'static Config =
        Config::from_file(&config_path).map(|config| Box::leak(Box::new(config)))?;

    if !config.start() {
        return Err("a start command failed, exiting.".into());
    }

    let mut stream_process_child_handles = Vec::new();
    let mut stream_task_handles = Vec::new();

    let (stream2match_tx, stream2match_rx) = mpsc::channel(123456);
    let (database2match_tx, database2match_rx) = mpsc::channel(234560);

    let (socket2match_tx, socket2match_rx) = mpsc::channel(1);
    let (socket2exec_tx, socket2exec_rx) = mpsc::channel(1);

    let (matches2exec_tx, matches2exec_rx) = mpsc::channel(234560);
    let (log_tx, log_rx) = mpsc::channel(234560);

    // Shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let matches_manager_task_handle = {
        let log_tx = log_tx.clone();
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            matches_manager(
                stream2match_rx,
                database2match_rx,
                socket2match_rx,
                matches2exec_tx,
                log_tx,
                shutdown_rx,
            )
            .await
        })
    };

    let execs_manager_task_handle = {
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            execs_manager(config, matches2exec_rx, socket2exec_rx, log_tx, shutdown_rx).await
        })
    };

    let database_manager_task_handle = {
        // The `task::spawn` is done in the function, after database rotation is finished
        database_manager(config, log_rx, database2match_tx).await
    };

    let socket_manager_task_handle = {
        let socket = socket.to_owned();
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            socket_manager(config, socket, socket2match_tx, socket2exec_tx, shutdown_rx).await
        })
    };

    for stream in config.streams().values() {
        let stream2match_tx = stream2match_tx.clone();
        let (child_tx, child_rx) = oneshot::channel();

        stream_task_handles.push(tokio::spawn(async move {
            stream.manager(child_tx, stream2match_tx).await
        }));

        if let Ok(Some(child)) = child_rx.await {
            stream_process_child_handles.push(child);
        }
    }
    drop(stream2match_tx);

    // Close streams when we receive a quit signal
    handle_signals(stream_process_child_handles, shutdown_tx.clone())?;

    // Wait for all streams to quit
    for task_handle in stream_task_handles {
        let _ = task_handle.await;
    }
    let _ = shutdown_tx.send(true);

    let _ = socket_manager_task_handle.await;
    let _ = matches_manager_task_handle.await;
    let _ = execs_manager_task_handle.await;
    let _ = database_manager_task_handle.await;

    let stop_ok = config.stop();

    // not waiting for the socket_manager to finish, sorry
    // TODO make it listen on shutdown_rx
    if let Err(err) = fs::remove_file(socket) {
        error!("failed to remove socket: {}", err);
    }

    if !*shutdown_rx.borrow() {
        Err("quitting because all streams finished".into())
    } else if !stop_ok {
        Err("while executing stop command".into())
    } else {
        Ok(())
    }
}

fn handle_signals(
    stream_process_child_handles: Vec<Child>,
    shutdown_tx: watch::Sender<bool>,
) -> tokio::io::Result<()> {
    let mut sighup = signal(SignalKind::hangup())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    tokio::spawn(async move {
        let signal = select! {
            _ = sighup.recv() => "SIGHUP",
            _ = sigint.recv() => "SIGINT",
            _ = sigterm.recv() => "SIGTERM",
        };
        let _ = shutdown_tx.send(true);
        info!("received {signal}, closing streams...");
        // Kill stream subprocesses
        for mut child_handle in stream_process_child_handles.into_iter() {
            tokio::spawn(async move { child_handle.kill().await });
        }
    });
    Ok(())
}
