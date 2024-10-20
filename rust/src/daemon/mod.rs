use std::{
    collections::BTreeMap,
    error::Error,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use socket::socket_manager;
use tokio::{
    process::Child,
    select,
    signal::unix::{signal, SignalKind},
    sync::{broadcast, mpsc, oneshot, Mutex, Semaphore},
};
use tracing::info;

use crate::concepts::{Config, Filter, Stream};
use database::database_manager;
use filter::FilterManager;
use stream::stream_manager;

mod database;
mod socket;

mod action;
mod filter;
mod stream;

// type SharedState = BTreeMap<&'static Stream, Arc<Mutex<BTreeMap<&'static Filter, FilterManager>>>>;
struct SharedState {
    pub s: BTreeMap<&'static Stream, Arc<Mutex<BTreeMap<&'static Filter, FilterManager>>>>,
}

#[allow(unsafe_code)]
// It's actually safe. It's a bug in rust that shadows the 'static reference, which ensures the
// reference will always link to something
// https://github.com/rust-lang/rust/issues/96865
unsafe impl Send for SharedState {}

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

    // let (socket2match_tx, socket2match_rx) = mpsc::channel(1);
    // let (socket2exec_tx, socket2exec_rx) = mpsc::channel(1);

    let (log_tx, log_rx) = mpsc::channel(234560);

    // Shutdown channel
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

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

    let mut stream_filter_managers = SharedState { s: BTreeMap::new() };
    for (stream, filter_manager_handlers) in stream_filter_managers_handlers {
        let mut filter_managers = BTreeMap::new();
        for (filter, filter_manager_handler) in filter_manager_handlers {
            filter_managers.insert(filter, filter_manager_handler.await.unwrap());
        }
        stream_filter_managers.s.insert(stream, Arc::new(Mutex::new(filter_managers)));
    }
    let stream_filter_managers: Arc<SharedState> = Arc::new(stream_filter_managers);

    for (stream, filter_managers) in stream_filter_managers.s.iter() {
        let (child_tx, child_rx) = oneshot::channel();
        let filter_managers = filter_managers.clone();
        let stream = *stream;

        stream_task_handles.push(tokio::spawn(async move {
            stream_manager(stream, child_tx, filter_managers).await
        }));

        if let Ok(Some(child)) = child_rx.await {
            stream_process_child_handles.push(child);
        }
    }

    // Close streams when we receive a quit signal
    let signal_received = Arc::new(AtomicBool::new(false));
    handle_signals(
        stream_process_child_handles,
        shutdown_tx.clone(),
        signal_received.clone(),
    )?;

    let socket_manager_task_handle = {
        let socket = socket.to_owned();
        let stream_filter_managers: Arc<SharedState> = stream_filter_managers.clone();
        tokio::spawn(async move {
            socket_manager(config, socket, stream_filter_managers, shutdown_rx).await
        })
    };

    // Wait for all streams to quit
    for task_handle in stream_task_handles {
        let _ = task_handle.await;
    }
    let _ = shutdown_tx.send(());

    let _ = socket_manager_task_handle.await;
    let _ = database_manager_thread_handle.join();

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
    stream_process_child_handles: Vec<Child>,
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
        let _ = shutdown_tx.send(());
        signal_received.store(true, Ordering::SeqCst);
        info!("received {signal}, closing streams...");
        // Kill stream subprocesses
        for mut child_handle in stream_process_child_handles.into_iter() {
            tokio::spawn(async move { child_handle.kill().await });
        }
    });
    Ok(())
}
