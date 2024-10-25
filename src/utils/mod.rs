use crate::shutdown::Shutdown;
use futures::future::Either;
use futures::{FutureExt, StreamExt};
use std::future::Future;
use std::time::Duration;
use tokio::select;
use tokio::task::JoinHandle;
use tracing::error;
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

/// Spawns a future that will retry on failure with a backoff duration
///
/// The future will retry until it succeeds or a shutdown signal is received.
/// During a shutdown, the task will be immediately cancelled
pub fn spawn_with_backoff_cancel_on_shutdown<S, F>(
    future_spawner: S,
    backoff_duration: Duration,
    shutdown: Shutdown,
) -> JoinHandle<()>
where
    F: Future<Output = anyhow::Result<()>> + Send + 'static,
    S: Fn() -> F + Send + Sync + 'static,
{
    // Run task in background, returning a handle.
    tokio::spawn(async move {
        select! {
            _ = retry_future(
                future_spawner,
                backoff_duration,
                &shutdown
            ) => {},
            _ = shutdown.await_shutdown_begin() => {},
        }
    })
}

/// Spawns a future that will retry on failure with a backoff duration
///
/// The future will retry until it succeeds, panics, or a shutdown signal is received.
/// During a shutdown, the task will be allowed to finish until the shutdown timout occurs.
/// This is useful if the task has custom cleanup logic that needs to be run.
pub fn spawn_with_backoff<S, F>(
    future_spawner: S,
    backoff_duration: Duration,
    shutdown: Shutdown,
) -> JoinHandle<()>
where
    F: Future<Output = anyhow::Result<()>> + Send + 'static,
    S: Fn() -> F + Send + Sync + 'static,
{
    // Run task in background, returning a handle.
    tokio::spawn(async move {
        let retry = Either::Left(retry_future(future_spawner, backoff_duration, &shutdown));
        let shutdown = Either::Right(shutdown.await_shutdown_begin());

        // If retry completes then we return
        // If shutdown completes then we still wait for retry
        futures::stream::iter([retry, shutdown])
            .buffered(2)
            .next()
            .await;
    })
}

/// Retries a future
///
/// The future will be polled on the current task. If the future returns an error,
/// the error will be logged and future will be retried after the backoff duration.
/// If the future panics the error will be logged and a shutdown signal will be sent.
async fn retry_future<S, F>(future_spawner: S, backoff_duration: Duration, shutdown: &Shutdown)
where
    F: Future<Output = anyhow::Result<()>> + Send + 'static,
    S: Fn() -> F + Send + Sync + 'static,
{
    loop {
        let future = future_spawner();

        // Wrap in `AssertUnwindSafe` so we can call `FuturesExt::catch_unwind` on it.
        let future = std::panic::AssertUnwindSafe(future);
        let result = future.catch_unwind().await;

        match result {
            // Task succeeded or is shutting down gracefully
            Ok(Ok(())) => return,
            Ok(Err(e)) => {
                error!("Task failed: {e:?}");

                if shutdown.is_shutting_down() {
                    return;
                }

                tokio::time::sleep(backoff_duration).await;
            }
            Err(e) => {
                error!("Task panicked: {e:?}");
                shutdown.shutdown();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    #[derive(Clone)]
    enum TaskResult {
        Ok,
        Error,
        Panic,
    }

    fn spawn_task(
        task_result: Arc<Mutex<TaskResult>>,
        got_err: Arc<AtomicBool>,
        shutdown: Shutdown,
    ) -> JoinHandle<()> {
        spawn_with_backoff(
            move || {
                let task_result = task_result.clone();
                let got_err = got_err.clone();

                async move {
                    match task_result.lock().unwrap().clone() {
                        TaskResult::Ok => Ok(()),
                        TaskResult::Error => {
                            got_err.store(true, Ordering::SeqCst);
                            Err(anyhow::anyhow!("Task failed"))
                        }
                        TaskResult::Panic => panic!("Panicking!"),
                    }
                }
            },
            Duration::from_millis(100),
            shutdown,
        )
    }

    #[tokio::test]
    async fn spawn_monitored_test() -> anyhow::Result<()> {
        let task_result = Arc::new(Mutex::new(TaskResult::Error));
        let got_err = Arc::new(AtomicBool::new(false));
        let shutdown = Shutdown::spawn(Duration::from_secs(30), Duration::from_millis(100));

        let handle = spawn_task(task_result.clone(), got_err.clone(), shutdown.clone());

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(got_err.load(Ordering::SeqCst));

        *task_result.lock().unwrap() = TaskResult::Ok;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(handle.is_finished(), "Task should be finished");

        *task_result.lock().unwrap() = TaskResult::Panic;
        let handle = spawn_task(task_result.clone(), got_err.clone(), shutdown.clone());

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(handle.is_finished(), "Task should be finished");

        tokio::time::sleep(Duration::from_millis(100)).await;
        panic!("The process should have exited already");
    }
}
