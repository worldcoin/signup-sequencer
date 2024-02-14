use eyre::Result;
use once_cell::sync::Lazy;
use tokio::sync::watch::{self, Receiver, Sender};
use tracing::info;

static NOTIFY: Lazy<(Sender<bool>, Receiver<bool>)> = Lazy::new(|| watch::channel(false));

/// Send the signal to shutdown the program.
pub fn shutdown() {
    // Does not fail because the channel never closes.
    NOTIFY.0.send(true).unwrap();
}

/// Reset the shutdown signal so it can be triggered again.
///
/// This is only useful for testing. Strange things can happen to any existing
/// `await_shutdown()` futures.
pub fn reset_shutdown() {
    // Does not fail because the channel never closes.
    NOTIFY.0.send(false).unwrap();
}

/// Are we currently shutting down?
#[must_use]
pub fn is_shutting_down() -> bool {
    *NOTIFY.1.borrow()
}

/// Wait for the program to shutdown.
///
/// Resolves immediately if the program is already shutting down.
/// The resulting future is safe to cancel by dropping.
pub async fn await_shutdown() {
    let mut watch = NOTIFY.1.clone();
    if *watch.borrow_and_update() {
        return;
    }
    // Does not fail because the channel never closes.
    watch.changed().await.unwrap();
}

pub fn watch_shutdown_signals() {
    tokio::spawn({
        async move {
            signal_shutdown()
                .await
                .map_err(|err| tracing::error!("Error handling Ctrl-C: {}", err))
                .unwrap();
            shutdown();
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

        tokio::spawn(async {
            sleep(Duration::from_millis(100)).await;
            shutdown();
        });

        await_shutdown().await;

        let elapsed = start.elapsed();

        assert!(elapsed > Duration::from_millis(100));
        assert!(elapsed < Duration::from_millis(200));
    }
}
