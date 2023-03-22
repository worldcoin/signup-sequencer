use anyhow::{Error as EyreError, Result as AnyhowResult};
use ethers::types::U256;
use futures::FutureExt;
use std::{future::Future, ptr};
use tokio::task::JoinHandle;
use tracing::error;

#[macro_export]
macro_rules! require {
    ($condition:expr, $err:expr) => {
        if !$condition {
            return Err($err);
        }
    };
}

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

/// Spawn a task and abort process if it panics or results in error.
pub fn spawn_or_abort<F, T>(future: F) -> JoinHandle<T>
where
    F: Future<Output = AnyhowResult<T>> + Send + 'static,
    T: Send + 'static,
{
    // Wrap in `AssertUnwindSafe` so we can call `FuturesExt::catch_unwind` on it.
    let future = std::panic::AssertUnwindSafe(future);

    // Run task in background, returning a handle.
    tokio::spawn(async move {
        let result = future.catch_unwind().await;
        match result {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                error!("Task failed: {:?}", e);
                std::process::abort();
            }
            Err(e) => {
                error!("Task panicked: {:?}", e);
                std::process::abort();
            }
        }
    })
}

#[allow(dead_code)]
pub fn u256_to_f64(value: U256) -> f64 {
    value.to_string().parse::<f64>().unwrap()
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
