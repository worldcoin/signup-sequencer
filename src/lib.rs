#![doc = include_str!("../Readme.md")]
#![warn(clippy::cargo)]
#![allow(clippy::multiple_crate_versions, clippy::too_many_arguments)]

pub mod app;
pub mod config;
mod contracts;
mod database;
mod ethereum;
mod identity;
pub mod identity_tree;
pub mod monitoring;
pub mod prover;
pub mod server;
pub mod shutdown;
pub mod task_monitor;
pub mod utils;
