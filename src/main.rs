#![doc = include_str!("../Readme.md")]
#![warn(clippy::cargo)]
#![allow(clippy::module_name_repetitions, clippy::wildcard_imports)]

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use cli_batteries::{run, version};
use signup_sequencer::app::App;
use signup_sequencer::config::Config;
use signup_sequencer::server;
use signup_sequencer::task_monitor::TaskMonitor;

#[derive(Debug, Clone, Parser)]
struct Args {
    /// Path to the optional config file
    config: Option<PathBuf>,
}

async fn app(args: Args) -> eyre::Result<()> {
    sequencer_app(args)
        .await
        .map_err(|e| eyre::eyre!("{:?}", e))
}

async fn sequencer_app(args: Args) -> anyhow::Result<()> {
    let mut settings = config::Config::builder();

    if let Some(path) = args.config {
        settings = settings.add_source(config::File::from(path).required(true));
    }

    let settings = settings
        .add_source(config::Environment::with_prefix("SEQ").separator("__"))
        .build()?;

    let config = settings.try_deserialize::<Config>()?;

    let server_config = config.server.clone();

    // Create App struct
    let app = Arc::new(App::new(config).await?);
    let app_for_server = app.clone();

    let task_monitor = TaskMonitor::new(app);

    // Process to push new identities to Ethereum
    task_monitor.start().await;

    // Start server (will stop on shutdown signal)
    server::main(app_for_server, server_config).await?;

    tracing::info!("Stopping the app");
    task_monitor.shutdown().await?;

    Ok(())
}

fn main() {
    run(version!(semaphore, ethers), app);
}
