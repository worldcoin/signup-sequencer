use std::future::Future;
use std::time::Duration;

use anyhow::{Error as EyreError, Result as AnyhowResult};
use futures::FutureExt;
use tokio::select;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{error, info};

pub trait Any<A> {
    fn any(self) -> AnyhowResult<A>;
}

impl<A, B> Any<A> for Result<A, B>
where
    B: Into<EyreError>,
{
    fn any(self) -> AnyhowResult<A> {
        self.map_err(Into::into)
    }
}

pub trait AnyFlatten<A> {
    fn any_flatten(self) -> AnyhowResult<A>;
}

impl<A, B, C> AnyFlatten<A> for Result<Result<A, B>, C>
where
    B: Into<EyreError>,
    C: Into<EyreError>,
{
    fn any_flatten(self) -> AnyhowResult<A> {
        self.map_err(Into::into)
            .and_then(|inner| inner.map_err(Into::into))
    }
}

pub fn spawn_monitored_with_backoff<S, F>(
    future_spawner: S,
    shutdown_sender: broadcast::Sender<()>,
    backoff_duration: Duration,
) -> JoinHandle<()>
where
    F: Future<Output = AnyhowResult<()>> + Send + 'static,
    S: Fn() -> F + Send + Sync + 'static,
{
    // Run task in background, returning a handle.
    tokio::spawn(async move {
        loop {
            let mut shutdown_receiver = shutdown_sender.subscribe();

            let future = future_spawner();

            // Wrap in `AssertUnwindSafe` so we can call `FuturesExt::catch_unwind` on it.
            let future = std::panic::AssertUnwindSafe(future);

            let result = select! {
                result = future.catch_unwind() => {
                    result
                }
                _ = shutdown_receiver.recv() => {
                    info!("Woke up by shutdown signal, exiting.");
                    return;
                }
            };

            // let result = future.catch_unwind().await;

            match result {
                // Task succeeded or is shutting down gracefully
                Ok(Ok(t)) => return t,
                Ok(Err(e)) => {
                    error!("Task failed: {e:?}");

                    if cli_batteries::is_shutting_down() {
                        std::process::abort();
                    }

                    tokio::time::sleep(backoff_duration).await;
                }
                Err(e) => {
                    error!("Task panicked: {e:?}");

                    if cli_batteries::is_shutting_down() {
                        std::process::abort();
                    }

                    tokio::time::sleep(backoff_duration).await;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn spawn_monitored_test() -> anyhow::Result<()> {
        let (shutdown_sender, _) = broadcast::channel(1);

        let can_finish = Arc::new(AtomicBool::new(false));
        let triggered_error = Arc::new(AtomicBool::new(false));

        let handle = {
            let can_finish = can_finish.clone();
            let triggered_error = triggered_error.clone();

            spawn_monitored_with_backoff(
                move || {
                    let can_finish = can_finish.clone();
                    let triggered_error = triggered_error.clone();

                    async move {
                        let can_finish = can_finish.load(Ordering::SeqCst);

                        if can_finish {
                            Ok(())
                        } else {
                            triggered_error.store(true, Ordering::SeqCst);

                            // Sleep a little to free up the executor
                            tokio::time::sleep(Duration::from_millis(20)).await;

                            panic!("Panicking!");
                        }
                    }
                },
                shutdown_sender,
                Duration::from_secs_f32(0.2),
            )
        };

        println!("Sleeping for 1 second");
        tokio::time::sleep(Duration::from_secs(1)).await;
        println!("Done sleeping");

        let has_triggered_error = triggered_error.load(Ordering::SeqCst);
        assert!(has_triggered_error);
        assert!(!handle.is_finished(), "Task should not be finished");

        can_finish.store(true, Ordering::SeqCst);
        triggered_error.store(false, Ordering::SeqCst);

        println!("Waiting for task to finish");
        drop(tokio::time::timeout(Duration::from_secs(1), handle).await?);

        let has_triggered_error = triggered_error.load(Ordering::SeqCst);
        // There is no code path that allows as to store false on the triggered error
        // Atomic so this should be always false
        assert!(!has_triggered_error);

        Ok(())
    }
}
