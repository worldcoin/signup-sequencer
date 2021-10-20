use anyhow::{Error as AnyError, Result as AnyResult};
use futures::FutureExt;
use std::future::Future;
use tokio::{spawn, task::JoinHandle};
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
    fn any(self) -> AnyResult<A>;
}

impl<A, B> Any<A> for Result<A, B>
where
    B: Into<AnyError>,
{
    fn any(self) -> AnyResult<A> {
        self.map_err(Into::into)
    }
}

pub trait AnyFlatten<A> {
    fn any_flatten(self) -> AnyResult<A>;
}

impl<A, B, C> AnyFlatten<A> for Result<Result<A, B>, C>
where
    B: Into<AnyError>,
    C: Into<AnyError>,
{
    fn any_flatten(self) -> AnyResult<A> {
        self.map_err(Into::into)
            .and_then(|inner| inner.map_err(Into::into))
    }
}

/// Spawn a task and abort process if it results in error.
/// Tasks must result in [`AnyResult<()>`]
pub fn spawn_or_abort<F>(future: F) -> JoinHandle<()>
where
    F: Future<Output = AnyResult<()>> + Send + 'static,
{
    spawn(future.map(|result| {
        if let Err(error) = result {
            // Log error
            error!(?error, "Error in task");
            // Abort process
            std::process::abort();
        }
    }))
}
