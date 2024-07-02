use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use tokio::select;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::shutdown::Shutdown;

pub mod batch_type;
pub mod index_packing;
pub mod min_map;
pub mod secret;
pub mod serde_utils;
pub mod tree_updates;

pub const TX_RETRY_LIMIT: u32 = 10;

/// Retries a transaction a certain number of times
/// Only errors originating from `Transaction::commit` are retried
/// Errors originating from the transaction function `$expression` are not
/// retried and are instead immediately rolled back.
///
/// # Example
/// ```ignore
/// let res = retry_tx!(db, tx, {
///     tx.execute("SELECT * FROM table").await?;
///     Ok(tx.execute("SELECT * FROM other").await?)
/// }).await;
#[macro_export]
macro_rules! retry_tx {
    ($pool:expr, $tx:ident, $expression:expr) => {
        async {
            let mut res;
            let mut counter = 0;
            loop {
                let mut $tx = $pool.begin().await?;
                res = async { $expression }.await;
                let limit = 10;
                if let Err(e) = res {
                    counter += 1;
                    if counter > limit {
                        return Err(e.into());
                    } else {
                        $tx.rollback().await?;
                        tracing::warn!(
                            error = ?e,
                            "db transaction returned error ({counter}/{limit})"
                        );
                        continue;
                    }
                }
                match $tx.commit().await {
                    Err(e) => {
                        counter += 1;
                        if counter > limit {
                            return Err(e.into());
                        } else {
                            tracing::warn!(
                                error = ?e,
                                "db transaction commit failed ({counter}/{limit})"
                            );
                        }
                    }
                    Ok(_) => break,
                }
            }
            res
        }
    };
}

pub fn spawn_monitored_with_backoff<S, F>(
    future_spawner: S,
    shutdown_sender: broadcast::Sender<()>,
    backoff_duration: Duration,
    shutdown: Arc<Shutdown>,
) -> JoinHandle<()>
where
    F: Future<Output = anyhow::Result<()>> + Send + 'static,
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

                    if shutdown.is_shutting_down() {
                        return;
                    }

                    tokio::time::sleep(backoff_duration).await;
                }
                Err(e) => {
                    error!("Task panicked: {e:?}");

                    if shutdown.is_shutting_down() {
                        return;
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
        let shutdown = Arc::new(Shutdown::new());
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
                shutdown,
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
