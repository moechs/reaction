use std::{
    collections::HashMap,
    error::Error,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures::future::join_all;
use reaction_plugin::shutdown::{ShutdownController, ShutdownDelegate, ShutdownToken};
use tokio::{
    select,
    signal::unix::{SignalKind, signal},
    sync::Semaphore,
    time::interval,
};
use tracing::{debug, error, info, warn};
use treedb::{Database, DatabaseRotate};

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

    // Cancel if DB fails
    let db_failed = Arc::new(AtomicBool::new(false));

    // Cancel when we receive a quit signal
    let signal_received = Arc::new(AtomicBool::new(false));
    if let Err(err) = handle_signals(shutdown.delegate(), signal_received.clone()) {
        error!("{err}");
        return 1;
    }

    let mut config_started = false;
    let mut daemon_err = false;

    // Start the real daemon 👹
    if let Err(err) = daemon_start(
        config,
        socket,
        shutdown.delegate(),
        shutdown.token(),
        &mut config_started,
        signal_received.clone(),
    )
    .await
    {
        error!("{err}");
        daemon_err = true;
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
    } else if !signal_received.load(Ordering::SeqCst) {
        error!("quitting because all streams finished");
        1
    } else if db_failed.load(Ordering::SeqCst) {
        error!("quitting because database errored");
        1
    } else {
        0
    }
}

// This part is in a separate function because a lot of its operations are fallible
// If one of them fails, reaction must not exit directly:
// it must properly shutdown all tasks before that.
// This function just uses ? to return, and its caller takes care of properly quitting.
async fn daemon_start(
    config: &'static Config,
    socket: PathBuf,
    shutdown_delegate: ShutdownDelegate,
    shutdown: ShutdownToken,
    config_started: &mut bool,
    db_failed: Arc<AtomicBool>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut plugins = Plugins::new(config, shutdown.clone()).await?;

    // Open Database
    let (cancellation, task_tracker) = shutdown.clone().split();
    let path = PathBuf::from(config.state_directory.clone());
    let mut db = Database::open(&path, cancellation, task_tracker).await?;
    let db_rotate = db.get_rotate()?;

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
                    &mut db,
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

    // Handle if DB fails
    handle_db(shutdown_delegate, db_failed, db);

    // Open socket and run task
    let socket = Socket::open(socket).await?;
    socket.manager(config, state, shutdown.clone(), db_rotate.clone());

    // all core systems started, we can run start commands
    *config_started = true;
    if !config.start() {
        return Err("a start command failed, exiting.".into());
    }

    // Finish plugin setup
    plugins.start().await?;
    plugins.manager();

    // Start db rotation at regular interval
    state_rotation(shutdown.clone(), config.state_rotation_interval, db_rotate);

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

fn state_rotation(
    shutdown: ShutdownToken,
    state_rotation_interval: Option<Duration>,
    mut db: DatabaseRotate,
) {
    if let Some(duration) = state_rotation_interval {
        let mut interval = interval(duration);
        tokio::spawn(async move {
            // skip the first interval, that ticks immediately
            interval.tick().await;
            // rotate at each tick until shutdown
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let _ = db.rotate().await;
                    }
                    _ = shutdown.wait() => {
                        break;
                    }
                }
            }
        });
    }
}

fn handle_db(shutdown: ShutdownDelegate, db_failed: Arc<AtomicBool>, db: Database) {
    tokio::spawn(async move {
        match db.error_rx.await {
            Ok(Ok(())) => return,
            Ok(Err(err)) => {
                error!("Database task failed: {err}");
            }
            Err(err) => {
                error!("Database task has quit without telling goodbye: {err}");
            }
        }
        warn!("set db_failed to true");
        db_failed.store(true, Ordering::SeqCst);
        warn!("asking shutdown");
        shutdown.ask_shutdown();
    });
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
        signal_received.store(true, Ordering::SeqCst);
        shutdown.ask_shutdown();
    });
    Ok(())
}
