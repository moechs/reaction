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

use crate::{concepts::Config, treedb::Database};
use filter::FilterManager;
pub use filter::React;
pub use shutdown::{ShutdownController, ShutdownDelegate, ShutdownToken};
use socket::Socket;
use stream::StreamManager;

#[cfg(test)]
pub use filter::tests;

mod filter;
mod shutdown;
mod socket;
mod stream;

pub async fn daemon(
    config_path: PathBuf,
    socket: PathBuf,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config: &'static Config = Box::leak(Box::new(Config::from_path(&config_path)?));

    // Cancellation Token
    let shutdown = ShutdownController::new();

    // Open Database
    let mut db = Database::open(config, shutdown.token()).await?;

    // Open Socket
    let socket = Socket::open(socket).await?;

    // reaction won't abort on startup anymore, we can run start commands
    if !config.start() {
        return Err("a start command failed, exiting.".into());
    }

    let (state, stream_managers) = {
        // Semaphore limiting action execution concurrency
        let exec_limit = match config.concurrency {
            0 => None,
            n => Some(Arc::new(Semaphore::new(n))),
        };

        // Filter managers
        let now = Local::now();
        let mut state = HashMap::new();
        let mut stream_managers = Vec::new();
        for stream in config.streams.values() {
            let mut filter_managers = HashMap::new();
            for filter in stream.filters.values() {
                let manager =
                    FilterManager::new(filter, exec_limit.clone(), shutdown.token(), &mut db, now).await?;
                filter_managers.insert(filter, manager);
            }
            state.insert(stream, filter_managers.clone());

            stream_managers.push(StreamManager::new(
                stream,
                filter_managers,
                shutdown.token(),
            )?);
        }
        (state, stream_managers)
    };

    // Run socket task
    socket.manager(config, state, shutdown.token());

    // Start Stream managers
    let mut stream_task_handles = Vec::new();
    for stream_manager in stream_managers {
        stream_task_handles.push(tokio::spawn(async move { stream_manager.start().await }));
    }

    // Close streams when we receive a quit signal
    let signal_received = Arc::new(AtomicBool::new(false));
    handle_signals(shutdown.delegate(), signal_received.clone())?;

    // Wait for all streams to quit
    for task_handle in stream_task_handles {
        let _ = task_handle.await;
    }

    // Release last db's sender
    let mut db_status = db.quit();

    debug!("Asking for all tasks to quit...");
    shutdown.ask_shutdown();

    debug!("Waiting for all tasks to quit...");
    shutdown.wait_shutdown().await;

    let stop_ok = config.stop();

    if let Ok(Err(err)) = db_status.try_recv() {
        Err(format!("database error: {}", err).into())
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
