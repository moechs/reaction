use std::{
    collections::HashMap,
    error::Error,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use chrono::Local;
use tokio::{
    select,
    signal::unix::{signal, SignalKind},
    sync::Semaphore,
};
use tracing::{debug, info};

use crate::concepts::Config;
use filter::FilterManager;
use fjallext::*;
pub use shutdown::{ShutdownController, ShutdownDelegate};
use socket::socket_manager;
use stream::stream_manager;

mod filter;
mod fjallext;
mod shutdown;
mod socket;
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

    // Cancellation Token
    let shutdown = ShutdownController::new();

    // Semaphore limiting action execution concurrency
    let exec_limit = match config.concurrency() {
        0 => None,
        n => Some(Arc::new(Semaphore::new(n))),
    };

    // Open Database
    let keyspace = open_db(config)?;

    // Filter managers
    let now = Local::now();
    let mut state = HashMap::new();
    for stream in config.streams().values() {
        let mut filter_managers = HashMap::new();
        for filter in stream.filters().values() {
            let manager =
                FilterManager::new(filter, exec_limit.clone(), shutdown.token(), &keyspace, now)?;
            filter_managers.insert(filter, manager);
        }
        state.insert(stream, filter_managers.clone());

        let token = shutdown.token();
        stream_task_handles.push(tokio::spawn(async move {
            stream_manager(stream, filter_managers, token).await
        }));
    }
    drop(exec_limit);

    // Close streams when we receive a quit signal
    let signal_received = Arc::new(AtomicBool::new(false));
    handle_signals(shutdown.delegate(), signal_received.clone())?;

    {
        let socket = socket.to_owned();
        let token = shutdown.token();
        tokio::spawn(async move { socket_manager(config, socket, state, token).await });
    }

    // Wait for all streams to quit
    for task_handle in stream_task_handles {
        let _ = task_handle.await;
    }

    debug!("Asking for all tasks to quit...");
    shutdown.ask_shutdown();

    debug!("Waiting for all tasks to quit...");
    shutdown.wait_shutdown().await;

    debug!("Closing DB...");
    let db_ok = keyspace.persist(fjall::PersistMode::SyncAll);

    let stop_ok = config.stop();

    if let Err(err) = db_ok {
        Err(format!("database was not saved, unrecoverable error: {}", err).into())
    } else if !signal_received.load(Ordering::SeqCst) {
        Err("quitting because all streams finished".into())
    } else if !stop_ok {
        Err("while executing stop command".into())
    } else {
        Ok(())
    }
}

fn handle_signals(
    shutdown: ShutdownDelegate,
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
        shutdown.ask_shutdown();
        signal_received.store(true, Ordering::SeqCst);
    });
    Ok(())
}
