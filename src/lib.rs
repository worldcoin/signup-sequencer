#![doc = include_str!("../Readme.md")]
#![warn(clippy::cargo)]
#![allow(clippy::module_name_repetitions, clippy::wildcard_imports, clippy::too_many_arguments)]

pub mod app;
mod contracts;
mod database;
mod ethereum;
pub mod identity_tree;
mod prover;
pub mod secret;
mod serde_utils;
pub mod server;
mod task_monitor;
pub mod utils;

use std::sync::Arc;

use anyhow::Result as AnyhowResult;
use clap::Parser;
use tracing::info;

use crate::app::App;

#[derive(Clone, Debug, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    #[clap(flatten)]
    pub app: app::Options,

    #[clap(flatten)]
    pub server: server::Options,
}

/// ```
/// assert!(true);
/// ```
#[allow(clippy::missing_errors_doc)]
pub async fn main(options: Options) -> AnyhowResult<()> {
    // Create App struct
    let app = Arc::new(App::new(options.app).await?);
    let app_for_server = app.clone();

    // Start server (will stop on shutdown signal)
    server::main(app_for_server, options.server).await?;

    info!("Stopping the app");
    app.shutdown().await?;

    Ok(())
}

#[cfg(test)]
pub mod test {
    use tracing::{error, warn};
    use tracing_test::traced_test;

    use super::*;

    #[test]
    #[allow(clippy::disallowed_methods)] // False positive from macro
    #[traced_test]
    fn test_with_log_output() {
        error!("logged on the error level");
        assert!(logs_contain("logged on the error level"));
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // False positive from macro
    #[traced_test]
    async fn async_test_with_log() {
        // Local log
        info!("This is being logged on the info level");

        // Log from a spawned task (which runs in a separate thread)
        tokio::spawn(async {
            warn!("This is being logged on the warn level from a spawned task");
        })
        .await
        .unwrap();

        // Ensure that `logs_contain` works as intended
        assert!(logs_contain("logged on the info level"));
        assert!(logs_contain("logged on the warn level"));
        assert!(!logs_contain("logged on the error level"));
    }
}
