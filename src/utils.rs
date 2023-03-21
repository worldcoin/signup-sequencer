use std::{future::Future, ptr};

use anyhow::{Error as EyreError, Result as AnyhowResult};
use ethers::types::U256;
use futures::FutureExt;
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

#[allow(dead_code)]
pub fn u256_to_f64(value: U256) -> f64 {
    value
        .to_string()
        .parse::<f64>()
        .expect("Failed to parse U256 to f64")
}

#[cfg(test)]
mod tests {
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
