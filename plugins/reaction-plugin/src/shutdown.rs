//! Helper module that provides structures to ease the quitting process when having multiple tokio tasks.
//!
//! It defines a [`ShutdownController`], that permits to keep track of ongoing tasks, ask them to shutdown and wait for all of them to quit.
//!
//! You can have it as an attribute of your plugin struct.
//! ```
//! struct MyPlugin {
//!   shutdown: ShutdownController
//! }
//! ```
//!
//! You can then give a [`ShutdownToken`] to other tasks when creating them:
//!
//! ```
//! impl PluginInfo for MyPlugin {
//!     async fn start(&mut self) -> RemoteResult<()> {
//!         let token = self.shutdown.token();
//!
//!         tokio::spawn(async move {
//!            token.wait().await;
//!            eprintln!("DEBUG shutdown asked to quit, now quitting")
//!         })
//!     }
//! }
//! ```
//!
//! On closing, calling [`ShutdownController::ask_shutdown`] will inform all tasks waiting on [`ShutdownToken::wait`] that it's time to leave.
//! Then we can wait for [`ShutdownController::wait_all_task_shutdown`] to complete.
//!
//! ```
//! impl PluginInfo for MyPlugin {
//!     async fn close(self) -> RemoteResult<()> {
//!         self.shutdown.ask_shutdown();
//!         self.shutdown.wait_all_task_shutdown().await;
//!         Ok(())
//!     }
//! }
//! ```
//!
//! [`ShutdownDelegate::handle_quit_signals`] permits to handle SIGHUP, SIGINT and SIGTERM by gracefully shutting down tasks.

use tokio::signal::unix::{SignalKind, signal};
use tokio_util::{
    sync::{CancellationToken, WaitForCancellationFuture},
    task::task_tracker::{TaskTracker, TaskTrackerToken},
};

/// Permits to keep track of ongoing tasks, ask them to shutdown and wait for all of them to quit.
/// Stupid wrapper around [`tokio_util::sync::CancellationToken`] and [`tokio_util::task::task_tracker::TaskTracker`].
#[derive(Default, Clone)]
pub struct ShutdownController {
    shutdown_notifyer: CancellationToken,
    task_tracker: TaskTracker,
}

impl ShutdownController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask for all tasks to quit
    pub fn ask_shutdown(&self) {
        self.shutdown_notifyer.cancel();
        self.task_tracker.close();
    }

    /// Wait for all tasks to quit.
    /// This task may return even without having called [`ShutdownController::ask_shutdown`]
    /// first, if all tasks quit by themselves.
    pub async fn wait_all_task_shutdown(self) {
        self.task_tracker.close();
        self.task_tracker.wait().await;
    }

    /// Returns a new shutdown token, to be held by a task.
    pub fn token(&self) -> ShutdownToken {
        ShutdownToken::new(self.shutdown_notifyer.clone(), self.task_tracker.token())
    }

    /// Returns a [`ShutdownDelegate`], which is able to ask for shutdown,
    /// without counting as a task that needs to be awaited.
    pub fn delegate(&self) -> ShutdownDelegate {
        ShutdownDelegate(self.shutdown_notifyer.clone())
    }

    /// Returns a future that will resolve only when a shutdown request happened.
    pub fn wait(&self) -> WaitForCancellationFuture<'_> {
        self.shutdown_notifyer.cancelled()
    }
}

/// Permits to ask for shutdown, without counting as a task that needs to be awaited.
pub struct ShutdownDelegate(CancellationToken);

impl ShutdownDelegate {
    /// Ask for all tasks to quit
    pub fn ask_shutdown(&self) {
        self.0.cancel();
    }

    /// Ensure [`Self::ask_shutdown`] is called whenever we receive SIGHUP,
    /// SIGTERM or SIGINT. Spawns a task that consumes self.
    pub fn handle_quit_signals(self) -> Result<(), String> {
        let err_str = |err| format!("could not register signal: {err}");

        let mut sighup = signal(SignalKind::hangup()).map_err(err_str)?;
        let mut sigint = signal(SignalKind::interrupt()).map_err(err_str)?;
        let mut sigterm = signal(SignalKind::terminate()).map_err(err_str)?;

        tokio::spawn(async move {
            let signal = tokio::select! {
                _ = sighup.recv() => "SIGHUP",
                _ = sigint.recv() => "SIGINT",
                _ = sigterm.recv() => "SIGTERM",
            };
            eprintln!("received {signal}, closing...");
            self.ask_shutdown();
        });
        Ok(())
    }
}

/// Created by a [`ShutdownController`].
/// Serves two purposes:
///
/// - Wait for a shutdown request to happen with [`Self::wait`]
/// - Keep track of the current task. While this token is held,
///   [`ShutdownController::wait_all_task_shutdown`] will block.
#[derive(Clone)]
pub struct ShutdownToken {
    shutdown_notifyer: CancellationToken,
    _task_tracker_token: TaskTrackerToken,
}

impl ShutdownToken {
    fn new(shutdown_notifyer: CancellationToken, _task_tracker_token: TaskTrackerToken) -> Self {
        Self {
            shutdown_notifyer,
            _task_tracker_token,
        }
    }

    /// Returns underlying [`CancellationToken`] and [`TaskTrackerToken`], consuming self.
    pub fn split(self) -> (CancellationToken, TaskTrackerToken) {
        (self.shutdown_notifyer, self._task_tracker_token)
    }

    /// Returns a future that will resolve only when a shutdown request happened.
    pub fn wait(&self) -> WaitForCancellationFuture<'_> {
        self.shutdown_notifyer.cancelled()
    }

    /// Returns true if the shutdown request happened
    pub fn is_shutdown(&self) -> bool {
        self.shutdown_notifyer.is_cancelled()
    }

    /// Ask for all tasks to quit
    pub fn ask_shutdown(&self) {
        self.shutdown_notifyer.cancel();
    }
}
