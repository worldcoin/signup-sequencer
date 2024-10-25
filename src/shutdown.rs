use std::time::Duration;

use eyre::Result;
use tokio::select;
use tokio::sync::watch::{self, Receiver, Sender};
use tracing::{error, info};

/// A watch channel used to syncronize tasks to shutdown.
#[derive(Clone)]
pub struct Shutdown {
    sender: Sender<bool>,
}

impl Shutdown {
    /// Create a new shutdown and spawn a task to monitor it.
    ///
    /// * `timeout` - The maximum time to wait for all receivers to be dropped.
    ///               If all receivers have not been dropped by the timout the
    ///               process will exit with code 1.
    ///
    /// * `delay` - A minimum delay before calling [`std::process::exit`]. This us useful to allow
    ///             for cancaled futures to make it to an await point.
    ///
    /// Shutdown can be triggered by calling [`Shutdown::shutdown`].
    /// The shutdown will begin immediately after the delay expires if no receivers
    /// are being held.
    pub fn spawn(timeout: Duration, delay: Duration) -> Self {
        let (sender, _) = watch::channel(false);
        let shutdown = Self { sender };
        shutdown.clone().spawn_monitor(timeout, delay);
        shutdown
    }

    /// Send the signal to shutdown the program.
    pub fn shutdown(&self) {
        self.sender.send(true).ok();
    }

    /// Are we currently shutting down?
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        *self.sender.subscribe().borrow()
    }

    /// Returns a `Receiver` which must be held until the caller is ready to shutdown.
    #[must_use]
    pub fn handle(&self) -> Receiver<bool> {
        self.sender.subscribe()
    }

    /// Wait for the shutdown to begin.
    ///
    /// Returns a `Receiver` which must be held until the caller is ready to shutdown.
    #[must_use]
    pub async fn await_shutdown_begin_with_handle(&self) -> Receiver<bool> {
        let mut receiver = self.sender.subscribe();
        if *receiver.borrow_and_update() {
            return receiver;
        }
        receiver.changed().await.ok();
        receiver
    }

    /// Wait for the shutdown to begin.
    ///
    /// If the caller down not already own a `Receiver`
    /// then the process may abort immediately when this returns.
    pub async fn await_shutdown_begin(&self) {
        let _ = self.await_shutdown_begin_with_handle().await;
    }

    /// Wait for the channel to be closed, signaling that all receivers have been dropped.
    pub async fn await_shutdown_complete(&self) {
        self.sender.closed().await;
    }

    /// Return the number of receivers still active.
    pub async fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Spawn a task that will monitor the shutdown signal.
    ///
    /// This should only be called once.
    /// Will force a shutdown with `std::process::exit(1)` if shutdown takes too long.
    /// Otherwise will exit with code 0.
    fn spawn_monitor(self, timeout: Duration, delay: Duration) {
        tokio::spawn(async move {
            select! {
                _ = self.await_shutdown_begin() => {
                    info!("shutdown monitor shutdown received");
                },
                _ = signal_shutdown() => {
                    self.shutdown();
                    info!("Shutdown signal received, shutting down");
                }
            }

            let start = tokio::time::Instant::now();
            tokio::time::sleep(delay).await;
            select! {
                _ = self.await_shutdown_complete() => {
                    let elapsed = start.elapsed();
                    info!("shutdown channel closed, gracefully shut down in {:?}", elapsed);
                    std::process::exit(0);
                },
                _ = tokio::time::sleep(timeout) => {
                    error!("shutdown monitor timed out");
                    std::process::exit(1);
                }
            }
        });
    }
}

#[cfg(unix)]
async fn signal_shutdown() -> Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let sigint = signal(SignalKind::interrupt())?;
    let sigterm = signal(SignalKind::terminate())?;
    tokio::pin!(sigint);
    tokio::pin!(sigterm);
    tokio::select! {
        _ = sigint.recv() => { info!("SIGINT received, shutting down"); }
        _ = sigterm.recv() => { info!("SIGTERM received, shutting down"); }
    };
    Ok(())
}

#[cfg(not(unix))]
async fn signal_shutdown() -> Result<()> {
    use tokio::signal::ctrl_c;

    ctrl_c().await?;
    info!("Ctrl-C received, shutting down");
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::time::{sleep, Duration};

    use super::*;

    #[tokio::test]
    async fn shutdown_signal() {
        let start = tokio::time::Instant::now();

        let shutdown = Shutdown::spawn(Duration::from_secs(30), Duration::from_secs(1));

        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(100)).await;
            shutdown_clone.shutdown();
        });

        shutdown.await_shutdown_begin().await;

        let elapsed = start.elapsed();

        assert!(elapsed > Duration::from_millis(100));
        assert!(elapsed < Duration::from_millis(200));
    }
}
