use std::{
    collections::HashMap,
    error::Error,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use futures::future::join_all;
use reaction_plugin::shutdown::{ShutdownController, ShutdownDelegate, ShutdownToken};
use tokio::{
    select,
    signal::unix::{SignalKind, signal},
    sync::Semaphore,
};
use tracing::{debug, error, info};
use treedb::Database;

use crate::concepts::{Config, now};
use filter::FilterManager;
pub use filter::React;
use plugin::Plugins;
use socket::Socket;
use stream::StreamManager;

#[cfg(test)]
pub use filter::tests;

mod filter;
mod plugin;
mod socket;
mod stream;
mod utils;

pub async fn daemon(config_path: PathBuf, socket: PathBuf) -> i32 {
    // Load config or quit
    let config: &'static Config = Box::leak(Box::new(match Config::from_path(&config_path) {
        Ok(config) => config,
        Err(err) => {
            error!("{err}");
            return 1;
        }
    }));

    // Cancellation Token
    let shutdown = ShutdownController::new();

    // Cancel when we receive a quit signal
    let signal_received = Arc::new(AtomicBool::new(false));
    if let Err(err) = handle_signals(shutdown.delegate(), signal_received.clone()) {
        error!("{err}");
        return 1;
    }

    let mut db = None;
    let mut config_started = false;
    let mut daemon_err = false;

    // Start the real daemon 👹
    if let Err(err) = daemon_start(
        config,
        socket,
        shutdown.token(),
        &mut db,
        &mut config_started,
    )
    .await
    {
        error!("{err}");
        daemon_err = true;
    }

    // Release last db's sender
    let mut db_status = None;
    if let Some(db) = db {
        db_status = Some(db.quit());
    }

    debug!("Asking for all tasks to quit...");
    shutdown.ask_shutdown();

    debug!("Waiting for all tasks to quit...");
    shutdown.wait_all_task_shutdown().await;

    let mut stop_ok = true;
    if config_started {
        stop_ok = config.stop();
    }

    if daemon_err || !stop_ok {
        1
    } else if let Some(mut db_status) = db_status
        && let Ok(Err(err)) = db_status.try_recv()
    {
        error!("database error: {}", err);
        1
    } else if !signal_received.load(Ordering::SeqCst) {
        error!("quitting because all streams finished");
        1
    } else {
        0
    }
}

async fn daemon_start(
    config: &'static Config,
    socket: PathBuf,
    shutdown: ShutdownToken,
    db: &mut Option<Database>,
    config_started: &mut bool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut plugins = Plugins::new(config, shutdown.clone()).await?;

    // Open Database
    let (cancellation, task_tracker) = shutdown.clone().split();
    let path = PathBuf::from(config.state_directory.clone());
    *db = Some(Database::open(&path, cancellation, task_tracker).await?);

    let (state, stream_managers) = {
        // Semaphore limiting action execution concurrency
        let exec_limit = match config.concurrency {
            0 => None,
            n => Some(Arc::new(Semaphore::new(n))),
        };

        // Filter managers
        let now = now();
        let mut state = HashMap::new();
        let mut stream_managers = Vec::new();
        for stream in config.streams.values() {
            let mut filter_managers = HashMap::new();
            for filter in stream.filters.values() {
                let manager = FilterManager::new(
                    filter,
                    exec_limit.clone(),
                    shutdown.clone(),
                    db.as_mut().unwrap(),
                    &mut plugins,
                    now,
                )
                .await?;
                filter_managers.insert(filter, manager);
            }
            state.insert(stream, filter_managers.clone());

            stream_managers.push(
                StreamManager::new(stream, filter_managers, shutdown.clone(), &mut plugins).await?,
            );
        }
        (state, stream_managers)
    };

    // Open socket and run task
    let socket = Socket::open(socket).await?;
    socket.manager(config, state, shutdown.clone());

    // all core systems started, we can run start commands
    *config_started = true;
    if !config.start() {
        return Err("a start command failed, exiting.".into());
    }

    // Finish plugin setup
    plugins.start().await?;
    plugins.manager();

    // Start Stream managers
    let stream_task_handles = stream_managers.into_iter().filter_map(|stream_manager| {
        let standalone = stream_manager.is_standalone();
        let handle = tokio::spawn(async move { stream_manager.start().await });
        // Only wait for standalone streams
        if standalone { Some(handle) } else { None }
    });

    // Wait for all streams to quit
    join_all(stream_task_handles).await;
    Ok(())
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
