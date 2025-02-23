use tokio::sync::mpsc;
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};

// Thanks to this article for inspiration
// https://www.wcygan.io/post/tokio-graceful-shutdown/
// Now TaskTracker exist, but I don't know what I'd gain for using it instead?
// https://docs.rs/tokio-util/0.7.13/tokio_util/task/task_tracker/struct.TaskTracker.html

pub struct ShutdownController {
    shutdown_notifyer: CancellationToken,
    task_tracker: mpsc::Sender<()>,
    task_waiter: mpsc::Receiver<()>,
}

impl ShutdownController {
    pub fn new() -> Self {
        let (task_tracker, task_waiter) = mpsc::channel(1);
        Self {
            shutdown_notifyer: CancellationToken::new(),
            task_tracker,
            task_waiter,
        }
    }

    pub fn ask_shutdown(&self) {
        self.shutdown_notifyer.cancel();
    }

    pub async fn wait_shutdown(mut self) {
        drop(self.task_tracker);
        self.task_waiter.recv().await;
    }

    pub fn token(&self) -> ShutdownToken {
        ShutdownToken::new(self.shutdown_notifyer.clone(), self.task_tracker.clone())
    }

    pub fn delegate(&self) -> ShutdownDelegate {
        ShutdownDelegate(self.shutdown_notifyer.clone())
    }
}

pub struct ShutdownDelegate(CancellationToken);
impl ShutdownDelegate {
    pub fn ask_shutdown(&self) {
        self.0.cancel();
    }
}

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

    pub fn wait(&self) -> WaitForCancellationFuture<'_> {
        self.shutdown_notifyer.cancelled()
    }
}
