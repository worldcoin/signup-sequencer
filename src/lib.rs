#![doc = include_str!("../Readme.md")]
#![warn(clippy::cargo)]
#![allow(
    clippy::module_name_repetitions,
    clippy::wildcard_imports,
    clippy::too_many_arguments
)]

pub mod app;
pub mod config;
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

#[allow(clippy::missing_errors_doc)]
pub async fn main(options: Options) -> anyhow::Result<()> {
    // Create App struct
    let app = Arc::new(App::new(options.app).await?);
    let app_for_server = app.clone();

    // Start server (will stop on shutdown signal)
    server::main(app_for_server, options.server).await?;

    info!("Stopping the app");
    app.shutdown().await?;

    Ok(())
}
