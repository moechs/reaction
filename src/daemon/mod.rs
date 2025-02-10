use std::{
    collections::HashMap,
    error::Error,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use tokio::{
    select,
    signal::unix::{signal, SignalKind},
    sync::{broadcast, Semaphore},
};
use tracing::info;

use crate::concepts::Config;
use filter::FilterManager;
use sledext::*;
use socket::socket_manager;
use stream::stream_manager;

mod sledext;
mod socket;

mod action;
mod filter;
mod stream;

pub async fn daemon(
    config_path: PathBuf,
    socket: PathBuf,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config: &'static Config = Box::leak(Box::new(Config::from_file(&config_path)?));

    if !config.start() {
        return Err("a start command failed, exiting.".into());
    }

    let mut stream_task_handles = Vec::new();

    // Shutdown channel
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Semaphore limiting action execution concurrency
    let exec_limit = if config.concurrency() > 0 {
        Some(Arc::new(Semaphore::new(config.concurrency())))
    } else {
        None
    };

    // Open Database
    let db = sled::open(format!("{}/data", config.state_directory()))?;

    // Filter managers
    let mut state = HashMap::new();
    for stream in config.streams().values() {
        let mut filter_managers = HashMap::new();
        for filter in stream.filters().values() {
            let manager = FilterManager::new(filter, exec_limit.clone(), &db)?;
            filter_managers.insert(filter, manager);
        }
        state.insert(stream, filter_managers.clone());

        let stream_shutdow_rx = shutdown_rx.resubscribe();
        stream_task_handles.push(tokio::spawn(async move {
            stream_manager(stream, filter_managers, stream_shutdow_rx).await
        }));
    }
    drop(exec_limit);

    // Close streams when we receive a quit signal
    let signal_received = Arc::new(AtomicBool::new(false));
    handle_signals(shutdown_tx.clone(), signal_received.clone())?;

    let socket_manager_task_handle = {
        let socket = socket.to_owned();
        tokio::spawn(async move { socket_manager(config, socket, state, shutdown_rx).await })
    };

    // Wait for all streams to quit
    for task_handle in stream_task_handles {
        let _ = task_handle.await;
    }
    let _ = shutdown_tx.send(());

    let _ = socket_manager_task_handle.await;

    let stop_ok = config.stop();

    if !signal_received.load(Ordering::SeqCst) {
        Err("quitting because all streams finished".into())
    } else if !stop_ok {
        Err("while executing stop command".into())
    } else {
        Ok(())
    }
}

fn handle_signals(
    shutdown_tx: broadcast::Sender<()>,
    signal_received: Arc<AtomicBool>,
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
        info!("received {signal}, closing streams...");
        let _ = shutdown_tx.send(());
        signal_received.store(true, Ordering::SeqCst);
    });
    Ok(())
}
