use std::{
    fmt::{self, Display, Formatter},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::timeout,
};

/// A read-write lock with timeout.
///
/// Wraps Tokio's [`RwLock`].
#[derive(Debug)]
pub struct TimedRwLock<T> {
    duration: Duration,
    inner:    RwLock<T>,
}

/// Error for [`TimedRwLock`].
#[derive(Debug, Error)]
#[error("Timeout while waiting for lock. Duration: {duration:?}, Operation: {operation}")]
pub struct Error {
    operation: Operation,
    duration:  Duration,
}

/// The kind of operation causing the error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Read,
    Write,
}

impl Display for Operation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => write!(f, "read"),
            Self::Write => write!(f, "write"),
        }
    }
}

impl<T> TimedRwLock<T> {
    pub fn new(duration: Duration, value: T) -> Self {
        Self::from_lock(duration, RwLock::new(value))
    }

    pub fn from_lock(duration: Duration, inner: RwLock<T>) -> Self {
        Self { duration, inner }
    }

    pub fn timeout(&self) -> Duration {
        self.duration
    }

    pub async fn read(&self) -> Result<RwLockReadGuard<'_, T>, Error> {
        timeout(self.duration, self.inner.read())
            .await
            .map_err(|_| Error {
                operation: Operation::Read,
                duration:  self.duration,
            })
    }

    pub async fn write(&self) -> Result<RwLockWriteGuard<'_, T>, Error> {
        timeout(self.duration, self.inner.write())
            .await
            .map_err(|_| Error {
                operation: Operation::Write,
                duration:  self.duration,
            })
    }
}
