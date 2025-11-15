use tokio_util::{
    sync::{CancellationToken, WaitForCancellationFuture},
    task::task_tracker::{TaskTracker, TaskTrackerToken},
};

/// Permits to keep track of ongoing tasks, ask them to shutdown and for all of them to quit.
/// Stupid wrapper around [`tokio_util::CancellationToken`] and [`tokio_util::task_tracker::TaskTracker`].
#[derive(Default)]
pub struct ShutdownController {
    shutdown_notifyer: CancellationToken,
    task_tracker: TaskTracker,
}

impl ShutdownController {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            shutdown_notifyer: CancellationToken::new(),
            task_tracker: TaskTracker::new(),
        }
    }

    /// Ask for all tasks to quit
    pub fn ask_shutdown(&self) {
        self.shutdown_notifyer.cancel();
        self.task_tracker.close();
    }

    /// Wait for all tasks to quit.
    /// This task may return even without having called [`ShutdownController::ask_shutdown`]
    /// first, if all tasks quit by themselves.
    pub async fn wait_shutdown(self) {
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
}

/// Permits to ask for shutdown, without counting as a task that needs to be awaited.
pub struct ShutdownDelegate(CancellationToken);

impl ShutdownDelegate {
    /// Ask for all tasks to quit
    pub fn ask_shutdown(&self) {
        self.0.cancel();
    }
}

/// Created by a [`ShutdownController`].
/// Serves two purposes:
///
/// - Wait for a shutdown request to happen with [`Self::wait`]
/// - Keep track of the current task. While this token is held,
///   [`ShutdownController::wait_shutdown`] will block.
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
