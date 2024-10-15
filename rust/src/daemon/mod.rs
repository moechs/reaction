use std::{collections::BTreeMap, error::Error, path::PathBuf, sync::Arc};

use tokio::{
    process::Child,
    select,
    signal::unix::{signal, SignalKind},
    sync::{mpsc, oneshot, watch, Semaphore},
};
use tracing::info;

use crate::concepts::Config;
use database::database_manager;
use filter::FilterManager;
use stream::stream_manager;

mod database;
// mod socket;

mod action;
mod filter;
mod stream;

pub async fn daemon(
    config_path: PathBuf,
    _socket: PathBuf,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config: &'static Config =
        Config::from_file(&config_path).map(|config| Box::leak(Box::new(config)))?;

    if !config.start() {
        return Err("a start command failed, exiting.".into());
    }

    let mut stream_process_child_handles = Vec::new();
    let mut stream_task_handles = Vec::new();

    // let (socket2match_tx, socket2match_rx) = mpsc::channel(1);
    // let (socket2exec_tx, socket2exec_rx) = mpsc::channel(1);

    let (log_tx, log_rx) = mpsc::channel(234560);

    // Shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Semaphore limiting action execution concurrency
    let exec_limit = if config.concurrency() > 0 {
        Some(Arc::new(Semaphore::new(config.concurrency())))
    } else {
        None
    };

    // Filter managers
    let mut stream_filter_managers_handlers = BTreeMap::new();
    let mut log2filter_tx = BTreeMap::new();
    for stream in config.streams().values() {
        let mut filter_managers_handlers = BTreeMap::new();
        for filter in stream.filters().values() {
            let manager = FilterManager::new(
                filter,
                BTreeMap::default(),
                exec_limit.clone(),
                log_tx.clone(),
            );
            let (tx, rx) = mpsc::channel(1);
            let handle = tokio::spawn(async move { manager.handle_db_entries(rx).await });
            filter_managers_handlers.insert(filter, handle);
            log2filter_tx.insert(filter, tx);
        }
        stream_filter_managers_handlers.insert(stream, filter_managers_handlers);
    }
    drop(log_tx);
    drop(exec_limit);

    let database_manager_thread_handle = {
        // The `task::spawn` is done in the function, after database rotation is finished
        database_manager(config, log_rx, log2filter_tx)
    };

    let mut stream_filter_managers = BTreeMap::new();
    for (stream, filter_manager_handlers) in stream_filter_managers_handlers {
        let mut filter_managers = BTreeMap::new();
        for (filter, filter_manager_handler) in filter_manager_handlers {
            filter_managers.insert(filter, filter_manager_handler.await.unwrap());
        }
        stream_filter_managers.insert(stream, filter_managers);
    }

    // let socket_manager_task_handle = {
    //     let socket = socket.to_owned();
    //     let shutdown_rx = shutdown_rx.clone();
    //     tokio::spawn(async move {
    //         socket_manager(config, socket, socket2match_tx, socket2exec_tx, shutdown_rx).await
    //     })
    // };

    for (stream, filter_managers) in stream_filter_managers {
        let (child_tx, child_rx) = oneshot::channel();

        stream_task_handles.push(tokio::spawn(async move {
            stream_manager(stream, child_tx, filter_managers.into_values().collect()).await
        }));

        if let Ok(Some(child)) = child_rx.await {
            stream_process_child_handles.push(child);
        }
    }

    // Close streams when we receive a quit signal
    handle_signals(stream_process_child_handles, shutdown_tx.clone())?;

    // Wait for all streams to quit
    for task_handle in stream_task_handles {
        let _ = task_handle.await;
    }
    let _ = shutdown_tx.send(true);

    // let _ = socket_manager_task_handle.await;
    let _ = database_manager_thread_handle.join();

    let stop_ok = config.stop();

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
