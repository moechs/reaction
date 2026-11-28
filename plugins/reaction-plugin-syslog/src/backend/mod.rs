use std::{future::Future, time::Duration};

pub mod tcp;
pub mod udp;

/// Sleep for `delay`, or return early when shutdown is requested.
pub(crate) async fn sleep_or_shutdown<F: Future<Output = ()>>(
    delay: Duration,
    shutdown: F,
) -> bool {
    let mut shutdown = std::pin::pin!(shutdown);
    tokio::select! {
        _ = &mut shutdown => true,
        _ = tokio::time::sleep(delay) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use reaction_plugin::shutdown::ShutdownController;
    use tokio::time::Instant;

    use super::*;

    #[tokio::test]
    async fn backoff_returns_on_shutdown() {
        let controller = ShutdownController::new();
        let token = controller.token();
        let closer = controller.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            closer.ask_shutdown();
        });

        let started = Instant::now();
        assert!(sleep_or_shutdown(Duration::from_secs(60), token.wait()).await);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn backoff_waits_without_shutdown() {
        assert!(!sleep_or_shutdown(Duration::from_millis(10), pending::<()>()).await);
    }
}
