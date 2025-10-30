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
use futures::future::join_all;
use tokio::{
    select,
    signal::unix::{signal, SignalKind},
    sync::Semaphore,
};
use tracing::{debug, info};

use crate::{concepts::Config, treedb::Database};
use filter::FilterManager;
pub use filter::React;
use plugin::Plugins;
pub use shutdown::{ShutdownController, ShutdownDelegate, ShutdownToken};
use socket::Socket;
use stream::StreamManager;

#[cfg(test)]
pub use filter::tests;

mod filter;
mod plugin;
mod shutdown;
mod socket;
mod stream;
mod utils;

pub async fn daemon(
    config_path: PathBuf,
    socket: PathBuf,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config: &'static Config = Box::leak(Box::new(Config::from_path(&config_path)?));

    // Cancellation Token
    let shutdown = ShutdownController::new();

    let mut plugins = Plugins::new(config, shutdown.token()).await?;

    // Open Database
    let mut db = Database::open(config, shutdown.token()).await?;

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
                let manager = FilterManager::new(
                    filter,
                    exec_limit.clone(),
                    shutdown.token(),
                    &mut db,
                    &mut plugins,
                    now,
                )
                .await?;
                filter_managers.insert(filter, manager);
            }
            state.insert(stream, filter_managers.clone());

            stream_managers.push(
                StreamManager::new(stream, filter_managers, shutdown.token(), &mut plugins).await?,
            );
        }
        (state, stream_managers)
    };

    // Finish plugin setup
    plugins.finish_setup().await?;
    plugins.manager();

    // Open socket and run task
    let socket = Socket::open(socket).await?;
    socket.manager(config, state, shutdown.token());

    // reaction won't abort on startup anymore, we can run start commands
    if !config.start() {
        return Err("a start command failed, exiting.".into());
    }

    // Start Stream managers
    let stream_task_handles = stream_managers.into_iter().filter_map(|stream_manager| {
        let standalone = stream_manager.is_standalone();
        let handle = tokio::spawn(async move { stream_manager.start().await });
        // Only wait for standalone streams
        if standalone {
            Some(handle)
        } else {
            None
        }
    });

    // Close streams when we receive a quit signal
    let signal_received = Arc::new(AtomicBool::new(false));
    handle_signals(shutdown.delegate(), signal_received.clone())?;

    // Wait for all streams to quit
    join_all(stream_task_handles).await;

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
