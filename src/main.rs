#![doc = include_str!("../Readme.md")]
#![warn(clippy::cargo)]
#![allow(
    clippy::module_name_repetitions,
    clippy::wildcard_imports,
    clippy::multiple_crate_versions
)]

use std::path::PathBuf;

use clap::Parser;
use signup_sequencer::app::App;
use signup_sequencer::config::{load_config, ServiceConfig};
use signup_sequencer::server;
use signup_sequencer::shutdown::Shutdown;
use signup_sequencer::task_monitor::TaskMonitor;
use telemetry_batteries::tracing::datadog::DatadogBattery;
use telemetry_batteries::tracing::stdout::StdoutBattery;
use telemetry_batteries::tracing::TracingShutdownHandle;

#[derive(Debug, Clone, Parser)]
struct Args {
    /// Path to the optional config file
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = Args::parse();
    sequencer_app(args)
        .await
        .map_err(|e| eyre::eyre!("{:?}", e))
}

async fn sequencer_app(args: Args) -> anyhow::Result<()> {
    let config = load_config(args.config.as_deref())?;

    let _tracing_shutdown_handle = init_telemetry(&config.service)?;

    let shutdown = Shutdown::spawn(config.app.shutdown_timeout, config.app.shutdown_delay);

    let version = env!("GIT_VERSION");

    tracing::info!(?config, version, "Starting the app");

    let server_config = config.server.clone();

    // Create App struct
    let app = App::new(config).await?;

    TaskMonitor::init(app.clone(), shutdown.clone()).await;

    // Start server (will stop on shutdown signal)
    server::run(app, server_config, shutdown.clone()).await?;

    // If server::run returns, we can shutdown the rest of the app
    tracing::info!("Shutting down");
    shutdown.shutdown();

    Ok(())
}

fn init_telemetry(service: &ServiceConfig) -> anyhow::Result<TracingShutdownHandle> {
    if let Some(ref datadog) = service.datadog {
        Ok(DatadogBattery::init(
            datadog.traces_endpoint.as_deref(),
            &service.service_name,
            None,
            true,
        ))
    } else {
        Ok(StdoutBattery::init())
    }
}
