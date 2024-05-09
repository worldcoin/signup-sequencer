// We include this module in multiple in multiple integration
// test crates - so some code may not be used in some cases
#![allow(dead_code, clippy::too_many_arguments, unused_imports)]

use ethers::types::U256;
use tracing::error;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::time::Uptime;

mod api;
pub mod docker_compose;

#[allow(unused)]
pub mod prelude {
    pub use std::time::Duration;

    pub use anyhow::{Context, Error};
    pub use hyper::client::HttpConnector;
    pub use hyper::{Body, Client, Request, StatusCode};
    pub use retry::delay::Fixed;
    pub use retry::retry;
    pub use serde_json::json;
    pub use tokio::spawn;
    pub use tokio::task::JoinHandle;
    pub use tracing::{error, info, instrument};
    pub use tracing_subscriber::fmt::format;

    pub use super::{generate_test_commitments, init_tracing_subscriber};
    pub use crate::common::api::{
        delete_identity, inclusion_proof, inclusion_proof_raw, insert_identity,
    };
}

/// Initializes the tracing subscriber.
///
/// Set the `QUIET_MODE` environment variable to reduce the complexity of the
/// log output.
pub fn init_tracing_subscriber() {
    let quiet_mode = std::env::var("QUIET_MODE").is_ok();
    let rust_log = std::env::var("RUST_LOG").unwrap_or("info".to_string());
    let result = if quiet_mode {
        tracing_subscriber::fmt()
            .with_env_filter(rust_log)
            .compact()
            .with_timer(Uptime::default())
            .try_init()
    } else {
        tracing_subscriber::fmt()
            .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
            .with_line_number(true)
            .with_env_filter(rust_log)
            .with_timer(Uptime::default())
            // .pretty()
            .try_init()
    };
    if let Err(error) = result {
        error!(error, "Failed to initialize tracing_subscriber");
    }
}

/// Generates identities for the purposes of testing. The identities are encoded
/// in hexadecimal as a string but without the "0x" prefix as required by the
/// testing utilities.
///
/// # Note
/// This utilises a significantly smaller portion of the 256-bit identities than
/// would be used in reality. This is both to make them easier to generate and
/// to ensure that we do not run afoul of the element numeric limit for the
/// snark scalar field.
pub fn generate_test_commitments(count: usize) -> Vec<String> {
    let mut commitments = vec![];

    for _ in 0..count {
        // Generate the identities using the just the last 64 bits (of 256) has so we're
        // guaranteed to be less than SNARK_SCALAR_FIELD.
        let bytes: [u8; 32] = U256::from(rand::random::<u64>()).into();
        let identity_string: String = hex::encode(bytes);

        commitments.push(identity_string);
    }

    commitments
}
