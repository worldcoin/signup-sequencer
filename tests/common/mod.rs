#![cfg(not(feature = "oz-provider"))]
// We include this module in multiple in multiple integration
// test crates - so some code may not be used in some cases
#![allow(dead_code)]

pub mod abi;
mod chain_mock;
mod prover_mock;

pub mod prelude {
    pub use std::time::Duration;

    pub use anyhow::Context;
    pub use clap::Parser;
    pub use cli_batteries::{reset_shutdown, shutdown};
    pub use ethers::abi::{AbiEncode, Address};
    pub use ethers::core::abi::Abi;
    pub use ethers::core::k256::ecdsa::SigningKey;
    pub use ethers::core::rand;
    pub use ethers::prelude::artifacts::{Bytecode, BytecodeObject};
    pub use ethers::prelude::{
        ContractFactory, Http, LocalWallet, NonceManagerMiddleware, Provider, Signer,
        SignerMiddleware, Wallet,
    };
    pub use ethers::providers::Middleware;
    pub use ethers::types::{Bytes, H256, U256};
    pub use ethers::utils::{Anvil, AnvilInstance};
    pub use hyper::client::HttpConnector;
    pub use hyper::{Body, Client, Request};
    pub use once_cell::sync::Lazy;
    pub use postgres_docker_utils::DockerContainerGuard;
    pub use semaphore::identity::Identity;
    pub use semaphore::merkle_tree::{self, Branch};
    pub use semaphore::poseidon_tree::{PoseidonHash, PoseidonTree};
    pub use semaphore::protocol::{self, generate_nullifier_hash, generate_proof};
    pub use semaphore::{hash_to_field, Field};
    pub use serde::{Deserialize, Serialize};
    pub use serde_json::json;
    pub use signup_sequencer::app::App;
    pub use signup_sequencer::identity_tree::Hash;
    pub use signup_sequencer::{server, Options};
    pub use tokio::spawn;
    pub use tokio::task::JoinHandle;
    pub use tracing::{error, info, instrument};
    pub use tracing_subscriber::fmt::format::FmtSpan;
    pub use tracing_subscriber::fmt::time::Uptime;
    pub use url::{Host, Url};

    pub use super::prover_mock::ProverService;
    pub use super::{
        abi as ContractAbi, generate_reference_proof_json, generate_test_identities,
        init_tracing_subscriber, spawn_app, spawn_deps, spawn_mock_prover, test_inclusion_proof,
        test_insert_identity, test_verify_proof, test_verify_proof_on_chain,
    };
}

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;

use futures::stream::FuturesUnordered;
use futures::StreamExt;
use hyper::StatusCode;

use self::chain_mock::{spawn_mock_chain, MockChain, SpecialisedContract};
use self::prelude::*;

#[allow(clippy::too_many_arguments)]
#[instrument(skip_all)]
pub async fn test_verify_proof(
    uri: &str,
    client: &Client<HttpConnector>,
    root: Field,
    signal_hash: Field,
    nullifier_hash: Field,
    external_nullifier_hash: Field,
    proof: protocol::Proof,
    expected_failure: Option<&str>,
) {
    let body = construct_verify_proof_body(
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
    );
    let req = Request::builder()
        .method("POST")
        .uri(uri.to_owned() + "/verifySemaphoreProof")
        .header("Content-Type", "application/json")
        .body(body)
        .expect("Failed to create verify proof hyper::Body");
    let mut response = client
        .request(req)
        .await
        .expect("Failed to execute request.");

    let bytes = hyper::body::to_bytes(response.body_mut())
        .await
        .expect("Failed to convert response body to bytes");
    let result = String::from_utf8(bytes.into_iter().collect())
        .expect("Could not parse response bytes to utf-8");

    if let Some(expected_failure) = expected_failure {
        assert!(!response.status().is_success());
        assert!(result.contains(expected_failure));
    } else {
        assert!(response.status().is_success());
    }
}

#[allow(clippy::too_many_arguments)]
#[instrument(skip_all)]
pub async fn test_verify_proof_on_chain(
    identity_manager: &SpecialisedContract,
    root: Field,
    signal_hash: Field,
    nullifier_hash: Field,
    external_nullifier_hash: Field,
    proof: protocol::Proof,
) -> anyhow::Result<()> {
    let root_tok: U256 = root.into();
    let signal_hash_tok: U256 = signal_hash.into();
    let nullifier_hash_tok: U256 = nullifier_hash.into();
    let external_nullifier_hash_tok: U256 = external_nullifier_hash.into();
    let proof_tok: [U256; 8] = match proof {
        protocol::Proof(ar, bs, krs) => {
            [ar.0, ar.1, bs.0[0], bs.0[1], bs.1[0], bs.1[1], krs.0, krs.1]
        }
    };
    let method = identity_manager.method::<_, ()>(
        "verifyProof",
        (
            root_tok,
            signal_hash_tok,
            nullifier_hash_tok,
            external_nullifier_hash_tok,
            proof_tok,
        ),
    )?;
    method.call().await?;

    Ok(())
}

#[instrument(skip_all)]
pub async fn test_inclusion_proof(
    uri: &str,
    client: &Client<HttpConnector>,
    leaf_index: usize,
    ref_tree: &PoseidonTree,
    leaf: &Hash,
    expect_failure: bool,
) {
    for i in 1..21 {
        let body = construct_inclusion_proof_body(leaf);
        info!(?uri, "Contacting");
        let req = Request::builder()
            .method("POST")
            .uri(uri.to_owned() + "/inclusionProof")
            .header("Content-Type", "application/json")
            .body(body)
            .expect("Failed to create inclusion proof hyper::Body");

        let mut response = client
            .request(req)
            .await
            .expect("Failed to execute request.");

        if expect_failure {
            assert!(!response.status().is_success());
            return;
        } else {
            assert!(response.status().is_success());
        }

        let bytes = hyper::body::to_bytes(response.body_mut())
            .await
            .expect("Failed to convert response body to bytes");
        let result = String::from_utf8(bytes.into_iter().collect())
            .expect("Could not parse response bytes to utf-8");
        let result_json = serde_json::from_str::<serde_json::Value>(&result)
            .expect("Failed to parse response as json");
        let status = result_json["status"]
            .as_str()
            .expect("Failed to get status");

        if status == "pending" {
            assert_eq!(
                result_json,
                generate_reference_proof_json(ref_tree, leaf_index, "pending")
            );
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            info!("Got pending, waiting 1 second, iteration {}", i);
            tokio::time::sleep(Duration::from_secs(1)).await;
        } else if status == "mined" || status == "processed" {
            // We don't differentiate between these 2 states in tests
            let proof_json = generate_reference_proof_json(ref_tree, leaf_index, status);
            assert_eq!(result_json, proof_json);
        } else {
            panic!("Unexpected status: {}", status);
        }
    }
}

#[instrument(skip_all)]
pub async fn test_add_batch_size(
    uri: impl Into<String>,
    prover_url: impl Into<String>,
    batch_size: u64,
    client: &Client<HttpConnector>,
) -> anyhow::Result<()> {
    let prover_url_string: String = prover_url.into();
    let body = Body::from(
        json!({
            "url": prover_url_string,
            "batchSize": batch_size,
            "timeoutSeconds": 3
        })
        .to_string(),
    );
    let request = Request::builder()
        .method("POST")
        .uri(uri.into() + "/addBatchSize")
        .header("Content-Type", "application/json")
        .body(body)
        .expect("Failed to create add batch size hyper::Body");

    client
        .request(request)
        .await
        .expect("Failed to execute request.");

    Ok(())
}

#[instrument(skip_all)]
pub async fn test_remove_batch_size(
    uri: impl Into<String>,
    batch_size: u64,
    client: &Client<HttpConnector>,
    expect_failure: bool,
) -> anyhow::Result<()> {
    let body = Body::from(json!({ "batchSize": batch_size }).to_string());
    let request = Request::builder()
        .method("POST")
        .uri(uri.into() + "/removeBatchSize")
        .header("Content-Type", "application/json")
        .body(body)
        .expect("Failed to create remove batch size hyper::Body");

    let mut result = client
        .request(request)
        .await
        .expect("Request didn't return.");

    let body_bytes = hyper::body::to_bytes(result.body_mut())
        .await
        .expect("Failed to get response bytes.");
    let body_str =
        String::from_utf8(body_bytes.into_iter().collect()).expect("Failed to decode response.");

    if expect_failure && body_str != "The last batch size cannot be removed" {
        anyhow::bail!("Expected failure, but got success");
    } else {
        Ok(())
    }
}

#[instrument(skip_all)]
pub async fn test_insert_identity(
    uri: &str,
    client: &Client<HttpConnector>,
    ref_tree: &mut PoseidonTree,
    test_leaves: &[Field],
    leaf_index: usize,
) -> (merkle_tree::Proof<PoseidonHash>, Field) {
    let body = construct_insert_identity_body(&test_leaves[leaf_index]);
    let req = Request::builder()
        .method("POST")
        .uri(uri.to_owned() + "/insertIdentity")
        .header("Content-Type", "application/json")
        .body(body)
        .expect("Failed to create insert identity hyper::Body");

    let mut response = client
        .request(req)
        .await
        .expect("Failed to execute request.");
    let bytes = hyper::body::to_bytes(response.body_mut())
        .await
        .expect("Failed to convert response body to bytes");
    if !response.status().is_success() {
        panic!("Failed to insert identity");
    }

    assert!(bytes.is_empty());
    ref_tree.set(leaf_index, test_leaves[leaf_index]);

    (ref_tree.proof(leaf_index).unwrap(), ref_tree.root())
}

fn construct_inclusion_proof_body(identity_commitment: &Hash) -> Body {
    Body::from(
        json!({
            "identityCommitment": identity_commitment,
        })
        .to_string(),
    )
}

fn construct_insert_identity_body(identity_commitment: &Field) -> Body {
    Body::from(
        json!({
            "identityCommitment": identity_commitment,
        })
        .to_string(),
    )
}

fn construct_verify_proof_body(
    root: Field,
    signal_hash: Field,
    nullifer_hash: Field,
    external_nullifier_hash: Field,
    proof: protocol::Proof,
) -> Body {
    Body::from(
        json!({
            "root": root,
            "signalHash": signal_hash,
            "nullifierHash": nullifer_hash,
            "externalNullifierHash": external_nullifier_hash,
            "proof": proof,
        })
        .to_string(),
    )
}

#[instrument(skip_all)]
pub async fn spawn_app(options: Options) -> anyhow::Result<(JoinHandle<()>, SocketAddr)> {
    let app = App::new(options.app).await.expect("Failed to create App");

    let ip: IpAddr = match options.server.server.host() {
        Some(Host::Ipv4(ip)) => ip.into(),
        Some(Host::Ipv6(ip)) => ip.into(),
        Some(_) => return Err(anyhow::anyhow!("Cannot bind {}", options.server.server)),
        None => Ipv4Addr::LOCALHOST.into(),
    };
    let port = options.server.server.port().unwrap_or(9998);
    let addr = SocketAddr::new(ip, port);
    let listener = TcpListener::bind(addr).expect("Failed to bind random port");
    let local_addr = listener.local_addr()?;

    let app = spawn({
        async move {
            info!("App thread starting");
            server::bind_from_listener(Arc::new(app), Duration::from_secs(30), listener)
                .await
                .expect("Failed to bind address");
            info!("App thread stopping");
        }
    });

    Ok((app, local_addr))
}

#[derive(Deserialize, Serialize, Debug)]
struct CompiledContract {
    abi:      Abi,
    bytecode: Bytecode,
}

pub async fn spawn_deps(
    initial_root: U256,
    batch_sizes: &[usize],
    tree_depth: u8,
) -> anyhow::Result<(
    MockChain,
    DockerContainerGuard,
    HashMap<usize, ProverService>,
    micro_oz::ServerHandle,
)> {
    let chain = spawn_mock_chain(initial_root, batch_sizes, tree_depth);
    let db_container = spawn_db();

    let prover_futures = FuturesUnordered::new();
    for batch_size in batch_sizes {
        prover_futures.push(spawn_mock_prover(*batch_size));
    }

    let (chain, db_container, provers) =
        tokio::join!(chain, db_container, prover_futures.collect::<Vec<_>>());

    let chain = chain?;

    let signing_key = SigningKey::from_bytes(chain.private_key.as_bytes())?;
    let micro_oz = micro_oz::spawn(chain.anvil.endpoint(), signing_key).await?;

    let provers = provers.into_iter().collect::<Result<Vec<_>, _>>()?;

    let prover_map = provers
        .into_iter()
        .map(|prover| (prover.batch_size(), prover))
        .collect();

    Ok((chain, db_container?, prover_map, micro_oz))
}

async fn spawn_db() -> anyhow::Result<DockerContainerGuard> {
    let db_container = postgres_docker_utils::setup().await.unwrap();

    Ok(db_container)
}

pub async fn spawn_mock_prover(batch_size: usize) -> anyhow::Result<ProverService> {
    let mock_prover_service = prover_mock::ProverService::new(batch_size).await?;

    Ok(mock_prover_service)
}

/// Initializes the tracing subscriber.
///
/// Set the `QUIET_MODE` environment variable to reduce the complexity of the
/// log output.
pub fn init_tracing_subscriber() {
    let quiet_mode = std::env::var("QUIET_MODE").is_ok();
    let result = if quiet_mode {
        tracing_subscriber::fmt()
            .with_env_filter("info,signup_sequencer=debug")
            .with_timer(Uptime::default())
            .try_init()
    } else {
        tracing_subscriber::fmt()
            .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
            .with_line_number(true)
            .with_env_filter("info,signup_sequencer=debug")
            .with_timer(Uptime::default())
            // .pretty()
            .try_init()
    };
    if let Err(error) = result {
        error!(error, "Failed to initialize tracing_subscriber");
    }
}

pub fn generate_reference_proof_json(
    ref_tree: &PoseidonTree,
    leaf_idx: usize,
    status: &str,
) -> serde_json::Value {
    let proof = ref_tree
        .proof(leaf_idx)
        .unwrap()
        .0
        .iter()
        .map(|branch| match branch {
            Branch::Left(hash) => json!({ "Left": hash }),
            Branch::Right(hash) => json!({ "Right": hash }),
        })
        .collect::<Vec<_>>();
    let root = ref_tree.root();
    json!({
        "status": status,
        "root": root,
        "proof": proof,
        "message": serde_json::Value::Null
    })
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
pub fn generate_test_identities(identity_count: usize) -> Vec<String> {
    let mut identities = vec![];

    for _ in 0..identity_count {
        // Generate the identities using the just the last 64 bits (of 256) has so we're
        // guaranteed to be less than SNARK_SCALAR_FIELD.
        let bytes: [u8; 32] = U256::from(rand::random::<u64>()).into();
        let identity_string: String = hex::encode(bytes);

        identities.push(identity_string);
    }

    identities
}
