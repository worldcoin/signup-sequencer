use std::{future::Future, ptr, time::Duration};

use anyhow::{Error as EyreError, Result as AnyhowResult};
use ethers::types::U256;
use futures::FutureExt;
use tokio::task::JoinHandle;
use tracing::error;

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

pub fn spawn_monitored_with_backoff<S, F, T>(
    future_spawner: S,
    backoff_duration: Duration,
) -> JoinHandle<T>
where
    F: Future<Output = AnyhowResult<T>> + Send + 'static,
    S: Fn() -> F + Send + Sync + 'static,
    T: Send + 'static,
{
    // Run task in background, returning a handle.
    tokio::spawn(async move {
        loop {
            let future = future_spawner();

            // Wrap in `AssertUnwindSafe` so we can call `FuturesExt::catch_unwind` on it.
            let future = std::panic::AssertUnwindSafe(future);

            let result = future.catch_unwind().await;

            match result {
                // Task succeeded or is shutting down gracefully
                Ok(Ok(t)) => return t,
                Ok(Err(e)) => {
                    error!("Task failed: {:?}", e);

                    if cli_batteries::is_shutting_down() {
                        std::process::abort();
                    }

                    tokio::time::sleep(backoff_duration).await;
                }
                Err(e) => {
                    error!("Task panicked: {:?}", e);

                    if cli_batteries::is_shutting_down() {
                        std::process::abort();
                    }

                    tokio::time::sleep(backoff_duration).await;
                }
            }
        }
    })
}

/// Enables a pattern of updating a value with a function that takes ownership
/// and returns a new version of the value. This is akin to `mem::replace`, but
/// allows more flexibility.
/// This call is unsafe if `modifier` panics. Therefore, all callers must ensure
/// that it does not happen.
pub fn replace_with<T>(value: &mut T, modifier: impl FnOnce(T) -> T) {
    unsafe {
        let v = ptr::read(value);
        let v = modifier(v);
        ptr::write(value, v);
    }
}

#[cfg(not(feature = "oz-provider"))]
pub fn u256_to_f64(value: U256) -> f64 {
    value
        .to_string()
        .parse::<f64>()
        .expect("Failed to parse U256 to f64")
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };

    use super::*;

    #[cfg(not(feature = "oz-provider"))]
    mod u256_to_f64_tests {
        use hex_literal::hex;
        use test_case::test_case;

        use super::*;

        #[test_case(1_000_000_000_000_000_000 => 1_000_000_000_000_000_000.0)]
        #[test_case(42 => 42.0)]
        #[test_case(0 => 0.0)]
        fn test_u256_to_f64_small(v: u64) -> f64 {
            u256_to_f64(U256::from(v))
        }

        #[test_case(hex!("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF") => 115792089237316195423570985008687907853269984665640564039457584007913129639935.0)]
        #[test_case(hex!("0FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF") => 7237005577332262213973186563042994240829374041602535252466099000494570602495.0)]
        fn test_u256_to_f64_large(v: [u8; 32]) -> f64 {
            u256_to_f64(U256::from(v))
        }
    }

    #[tokio::test]
    async fn exp_backoff_test() -> anyhow::Result<()> {
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
