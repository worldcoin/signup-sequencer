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
use signup_sequencer::config::{Config, MetricsConfig, ServiceConfig};
use signup_sequencer::server;
use signup_sequencer::shutdown::watch_shutdown_signals;
use signup_sequencer::task_monitor::TaskMonitor;
use telemetry_batteries::metrics::prometheus::PrometheusBattery;
use telemetry_batteries::metrics::statsd::StatsdBattery;
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
    let config = load_config(&args)?;

    let _ = init_telemetry(&config.service)?;

    watch_shutdown_signals();

    let server_config = config.server.clone();

    // Create App struct
    let app = App::new(config).await?;

    let task_monitor = TaskMonitor::new(app.clone());

    // Process to push new identities to Ethereum
    task_monitor.start().await;

    // Start server (will stop on shutdown signal)
    server::run(app, server_config).await?;

    tracing::info!("Stopping the app");
    task_monitor.shutdown().await?;

    Ok(())
}

fn load_config(args: &Args) -> anyhow::Result<Config> {
    let mut settings = config::Config::builder();

    if let Some(ref path) = args.config {
        settings = settings.add_source(config::File::from(path.clone()).required(true));
    }

    let settings = settings
        .add_source(
            config::Environment::with_prefix("SEQ")
                .separator("__")
                .try_parsing(true),
        )
        .build()?;

    Ok(settings.try_deserialize::<Config>()?)
}

fn init_telemetry(service: &ServiceConfig) -> anyhow::Result<TracingShutdownHandle> {
    match service.metrics.clone() {
        Some(MetricsConfig::Prometheus(prometheus)) => {
            PrometheusBattery::init(Some(prometheus))?;
        }
        Some(MetricsConfig::Statsd(statsd)) => {
            StatsdBattery::init(
                &statsd.metrics_host,
                statsd.metrics_port,
                statsd.metrics_queue_size,
                statsd.metrics_buffer_size,
                Some(&statsd.metrics_prefix),
            )?;
        }
        _ => {}
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_example_env() {
        dotenv::from_path("example.env").ok();
        let args = Args { config: None };
        load_config(&args).unwrap();
    }
}
