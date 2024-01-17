#![doc = include_str!("../Readme.md")]
#![warn(clippy::cargo)]
#![allow(
    // clippy::module_name_repetitions,
    // clippy::wildcard_imports,
    clippy::too_many_arguments
)]

pub mod app;
pub mod config;
mod contracts;
mod database;
mod ethereum;
pub mod identity_tree;
pub mod prover;
pub mod secret;
mod serde_utils;
pub mod server;
pub mod task_monitor;
pub mod utils;
