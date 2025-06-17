use tokio::sync::mpsc;
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};

// Thanks to this article for inspiration
// https://www.wcygan.io/post/tokio-graceful-shutdown/
// Now TaskTracker exist, but I don't know what I'd gain for using it instead?
// https://docs.rs/tokio-util/0.7.13/tokio_util/task/task_tracker/struct.TaskTracker.html

/// Permits to keep track of ongoing tasks and ask them to shutdown.
pub struct ShutdownController {
    shutdown_notifyer: CancellationToken,
    task_tracker: mpsc::Sender<()>,
    task_waiter: mpsc::Receiver<()>,
}

impl ShutdownController {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let (task_tracker, task_waiter) = mpsc::channel(1);
        Self {
            shutdown_notifyer: CancellationToken::new(),
            task_tracker,
            task_waiter,
        }
    }

    /// Ask for all tasks to quit
    pub fn ask_shutdown(&self) {
        self.shutdown_notifyer.cancel();
    }

    /// Wait for all tasks to quit.
    /// This task may return even without having called [`ShutdownController::ask_shutdown`]
    /// first, if all tasks quit by themselves.
    pub async fn wait_shutdown(mut self) {
        drop(self.task_tracker);
        self.task_waiter.recv().await;
    }

    /// Returns a new shutdown token, to be held by a task.
    pub fn token(&self) -> ShutdownToken {
        ShutdownToken::new(self.shutdown_notifyer.clone(), self.task_tracker.clone())
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
/// - Wait for a shutdown request to happen.
/// - Keep track of the current task. While this token is held,
///   the [`ShutdownController::wait_shutdown`] will block.
#[derive(Clone)]
pub struct ShutdownToken {
    shutdown_notifyer: CancellationToken,
    _task_tracker: mpsc::Sender<()>,
}

impl ShutdownToken {
    fn new(shutdown_notifyer: CancellationToken, _task_tracker: mpsc::Sender<()>) -> Self {
        Self {
            shutdown_notifyer,
            _task_tracker,
        }
    }

    /// Returns a future that will resolve only when a shutdown request happened.
    pub fn wait(&self) -> WaitForCancellationFuture<'_> {
        self.shutdown_notifyer.cancelled()
    }

    /// Ask for all tasks to quit
    pub fn ask_shutdown(&self) {
        self.shutdown_notifyer.cancel();
    }
}
