// We include this module in multiple in multiple integration
// test crates - so some code may not be used in some cases
#![allow(dead_code, clippy::too_many_arguments, unused_imports)]

use std::time::Duration;

use anyhow::anyhow;
use ethers::providers::Middleware;
use ethers::types::U256;
use hyper::client::HttpConnector;
use hyper::Client;
use serde_json::Value;
use signup_sequencer::identity_tree::ProcessedStatus::Mined;
use signup_sequencer::identity_tree::{Hash, Status};
use signup_sequencer::server::api_v1::data::InclusionProofResponse;
use tokio::time::sleep;
use tracing::{error, info};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::time::Uptime;

use crate::common::abi::RootInfo;
use crate::common::api::{delete_identity, inclusion_proof, inclusion_proof_raw, insert_identity};
use crate::common::chain::Chain;
use crate::common::prelude::StatusCode;

mod abi;
mod api;
pub mod chain;
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

    pub use super::{
        bad_request_inclusion_proof_with_retries, delete_identity_with_retries,
        generate_test_commitments, init_tracing_subscriber, insert_identity_with_retries,
        mined_inclusion_proof_with_retries,
    };
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
pub fn generate_test_commitments(count: usize) -> Vec<Hash> {
    (0..count)
        .map(|_| Hash::from(rand::random::<u64>()))
        .collect()
}

pub async fn delete_identity_with_retries(
    client: &Client<HttpConnector>,
    uri: &String,
    commitment: &Hash,
    retries_count: usize,
    retries_interval: f32,
) -> anyhow::Result<()> {
    let mut last_err = None;

    for _ in 0..retries_count {
        match delete_identity(client, uri, commitment).await {
            Ok(_) => return Ok(()),
            Err(err) => last_err = Some(err),
        }
        sleep(Duration::from_secs_f32(retries_interval)).await;
    }

    Err(last_err.unwrap_or_else(|| anyhow!("All retries failed without error")))
}

pub async fn insert_identity_with_retries(
    client: &Client<HttpConnector>,
    uri: &String,
    commitment: &Hash,
    retries_count: usize,
    retries_interval: f32,
) -> anyhow::Result<()> {
    let mut last_err = None;
    for _ in 0..retries_count {
        match insert_identity(client, uri, commitment).await {
            Ok(_) => return Ok(()),
            Err(err) => last_err = Some(err),
        }
        _ = sleep(Duration::from_secs_f32(retries_interval)).await;
    }

    Err(last_err.unwrap_or_else(|| anyhow!("All retries failed without error")))
}

pub async fn mined_inclusion_proof_with_retries(
    client: &Client<HttpConnector>,
    uri: &String,
    chain: &Chain,
    commitment: &Hash,
    retries_count: usize,
    retries_interval: f32,
    offchain_mode: bool,
) -> anyhow::Result<()> {
    let mut last_res = None;
    for _i in 0..retries_count {
        last_res = Some(inclusion_proof(client, uri, commitment).await?);

        if let Some((status_code, ref inclusion_proof_json)) = last_res {
            if status_code.is_success() {
                if let Some(inclusion_proof_json) = inclusion_proof_json {
                    if offchain_mode {
                        if inclusion_proof_json.status == Status::Processed(Mined) {
                            return Ok(());
                        }
                    } else if let Some(root) = inclusion_proof_json.root {
                        let (root, ..) = chain
                            .identity_manager
                            .query_root(root.into())
                            .call()
                            .await?;

                        if root != U256::zero() {
                            return Ok(());
                        }
                    }
                }
            }
        };

        _ = sleep(Duration::from_secs_f32(retries_interval)).await;
    }

    if last_res.is_none() {
        return Err(anyhow!("No calls at all"));
    }

    if offchain_mode {
        Err(anyhow!("Inclusion proof with status mined not found"))
    } else {
        Err(anyhow!("Inclusion proof not found on chain"))
    }
}

pub async fn bad_request_inclusion_proof_with_retries(
    client: &Client<HttpConnector>,
    uri: &String,
    commitment: &Hash,
    retries_count: usize,
    retries_interval: f32,
) -> anyhow::Result<()> {
    let mut last_err = None;

    for _ in 0..retries_count {
        match inclusion_proof_raw(client, uri, commitment).await {
            Ok(response) if response.status_code == StatusCode::BAD_REQUEST => return Ok(()),
            Err(err) => {
                error!("error: {}", err);
                last_err = Some(err);
            }
            _ => {}
        }
        sleep(Duration::from_secs_f32(retries_interval)).await;
    }

    Err(last_err.unwrap_or_else(|| anyhow!("All retries failed to return BAD_REQUEST")))
}
