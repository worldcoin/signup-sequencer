use std::{
    fmt::{self, Display, Formatter},
    ops::{Deref, DerefMut},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::timeout,
};

// FEATURE: Add tracing spans to wait and the guard.

/// A read-write lock with timeout.
///
/// Wraps Tokio's [`RwLock`].
#[derive(Debug)]
pub struct TimedReadProgressLock<T: Send + Sync> {
    duration:       Duration,
    rw_lock:        Arc<RwLock<T>>,
    progress_mutex: Mutex<()>,
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
    Progress,
    Write,
}

impl Display for Operation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => write!(f, "read"),
            Self::Write => write!(f, "write"),
            Self::Progress => write!(f, "progress"),
        }
    }
}

impl<T: Send + Sync> TimedReadProgressLock<T> {
    pub fn new(duration: Duration, value: T) -> Self {
        Self {
            duration,
            rw_lock: Arc::new(RwLock::new(value)),
            progress_mutex: Mutex::new(()),
        }
    }

    pub async fn read(&self) -> Result<RwLockReadGuard<'_, T>, Error> {
        timeout(self.duration, self.rw_lock.read())
            .await
            .map_err(|_| Error {
                operation: Operation::Read,
                duration:  self.duration,
            })
    }

    pub async fn progress(&self) -> Result<ProgressGuard<'_, T>, Error> {
        timeout(self.duration, async {
            let mutex_guard = self.progress_mutex.lock().await;
            let resource_read_guard = self.rw_lock.read().await;
            ProgressGuard {
                duration: self.duration,
                mutex_guard,
                resource_read_guard,
                resource_lock: self.rw_lock.clone(),
            }
        })
        .await
        .map_err(|_| Error {
            operation: Operation::Progress,
            duration:  self.duration,
        })
    }

    pub async fn write(&self) -> Result<WriteGuard<'_, T>, Error> {
        timeout(self.duration, async {
            let mutex_guard = self.progress_mutex.lock().await;
            let write_guard = self.rw_lock.write().await;
            WriteGuard {
                mutex_guard,
                write_guard,
            }
        })
        .await
        .map_err(|_| Error {
            operation: Operation::Write,
            duration:  self.duration,
        })
    }
}

struct ProgressGuard<'a, T> {
    duration:            Duration,
    mutex_guard:         MutexGuard<'a, ()>,
    resource_read_guard: RwLockReadGuard<'a, T>,
    resource_lock:       Arc<RwLock<T>>,
}

impl<'a, T> ProgressGuard<'a, T> {
    pub async fn enter_critical(self) -> Result<WriteGuard<'a, T>, Error> {
        drop(self.resource_read_guard);
        let duration = self.duration;
        let mutex_guard = self.mutex_guard;
        let resource_lock = self.resource_lock.clone();
        timeout(self.duration, async move {
            let write_guard = resource_lock.write().await;
            WriteGuard {
                mutex_guard,
                write_guard,
            }
        })
        .await
        .map_err(|_| Error {
            operation: Operation::Write,
            duration,
        })
    }
}

impl<'a, T> Deref for ProgressGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &*self.resource_read_guard
    }
}

struct WriteGuard<'a, T> {
    mutex_guard: MutexGuard<'a, ()>,
    write_guard: RwLockWriteGuard<'a, T>,
}

impl<'a, T> Deref for WriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &*self.write_guard
    }
}

impl<'a, T> DerefMut for WriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.write_guard
    }
}
