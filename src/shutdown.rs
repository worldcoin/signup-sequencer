use std::sync::Arc;

use eyre::Result;
use tokio::sync::watch::{self, Receiver, Sender};
use tracing::info;

pub struct Shutdown {
    sender:   Sender<bool>,
    receiver: Receiver<bool>,
}

impl Shutdown {
    pub fn new() -> Self {
        let (sender, receiver) = watch::channel(false);
        Self { sender, receiver }
    }

    /// Send the signal to shutdown the program.
    pub fn shutdown(&self) {
        // Does not fail because the channel never closes.
        self.sender.send(true).unwrap();
    }

    /// Are we currently shutting down?
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        *self.receiver.borrow()
    }

    /// Wait for the program to shutdown.
    ///
    /// Resolves immediately if the program is already shutting down.
    /// The resulting future is safe to cancel by dropping.
    pub async fn await_shutdown(&self) {
        let mut watch = self.receiver.clone();
        if *watch.borrow_and_update() {
            return;
        }
        // Does not fail because the channel never closes test_config.
        watch.changed().await.unwrap();
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

pub fn watch_shutdown_signals(shutdown: Arc<Shutdown>) {
    tokio::spawn({
        async move {
            signal_shutdown()
                .await
                .map_err(|err| tracing::error!("Error handling Ctrl-C: {}", err))
                .unwrap();
            shutdown.shutdown();
        }
    });
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

        let shutdown = Arc::new(Shutdown::new());

        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(100)).await;
            shutdown_clone.shutdown();
        });

        shutdown.await_shutdown().await;

        let elapsed = start.elapsed();

        assert!(elapsed > Duration::from_millis(100));
        assert!(elapsed < Duration::from_millis(200));
    }
}
