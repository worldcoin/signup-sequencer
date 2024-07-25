// We include this module in multiple in multiple integration
// test crates - so some code may not be used in some cases
#![allow(dead_code, clippy::too_many_arguments, unused_imports)]

pub mod abi;
mod chain_mock;
mod prover_mock;
pub mod test_config;

#[allow(unused)]
pub mod prelude {
    pub use std::time::Duration;

    pub use anyhow::Context;
    pub use clap::Parser;
    pub use ethers::abi::{AbiEncode, Address};
    pub use ethers::core::abi::Abi;
    pub use ethers::core::k256::ecdsa::SigningKey;
    pub use ethers::core::rand;
    pub use ethers::prelude::{
        ContractFactory, Http, LocalWallet, NonceManagerMiddleware, Provider, Signer,
        SignerMiddleware, Wallet,
    };
    pub use ethers::providers::Middleware;
    pub use ethers::types::{Bytes, H256, U128, U256};
    pub use ethers::utils::{Anvil, AnvilInstance};
    pub use ethers_solc::artifacts::{Bytecode, BytecodeObject};
    pub use hyper::client::HttpConnector;
    pub use hyper::{Body, Client, Request};
    pub use once_cell::sync::Lazy;
    pub use postgres_docker_utils::DockerContainer;
    pub use semaphore::identity::Identity;
    pub use semaphore::merkle_tree::{self, Branch};
    pub use semaphore::poseidon_tree::{PoseidonHash, PoseidonTree};
    pub use semaphore::protocol::{self, generate_nullifier_hash, generate_proof};
    pub use semaphore::{hash_to_field, Field};
    pub use serde::{Deserialize, Serialize};
    pub use serde_json::json;
    pub use signup_sequencer::app::App;
    pub use signup_sequencer::config::{
        AppConfig, Config, DatabaseConfig, OzDefenderConfig, ProvidersConfig, RelayerConfig,
        ServerConfig, TreeConfig, TxSitterConfig,
    };
    pub use signup_sequencer::identity_tree::{Hash, TreeVersionReadOps};
    pub use signup_sequencer::prover::ProverType;
    pub use signup_sequencer::server;
    pub use signup_sequencer::shutdown::Shutdown;
    pub use testcontainers::clients::Cli;
    pub use tokio::spawn;
    pub use tokio::task::JoinHandle;
    pub use tracing::{error, info, instrument};
    pub use tracing_subscriber::fmt::format::FmtSpan;
    pub use tracing_subscriber::fmt::time::Uptime;
    pub use url::{Host, Url};

    pub use super::prover_mock::ProverService;
    pub use super::test_config::{
        self, TestConfigBuilder, DEFAULT_BATCH_DELETION_TIMEOUT_SECONDS,
        DEFAULT_TREE_DENSE_PREFIX_DEPTH, DEFAULT_TREE_DEPTH,
    };
    pub use super::{
        abi as ContractAbi, generate_reference_proof, generate_test_identities,
        init_tracing_subscriber, spawn_app, spawn_deps, spawn_mock_deletion_prover,
        spawn_mock_insertion_prover, test_inclusion_proof, test_insert_identity, test_verify_proof,
        test_verify_proof_on_chain,
    };
    pub use crate::common::chain_mock::spawn_mock_chain;
    pub use crate::common::test_same_tree_states;
}

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::str::FromStr;
use std::sync::{Arc, Once};

use futures::stream::FuturesUnordered;
use futures::StreamExt;
use hyper::StatusCode;
use semaphore::poseidon_tree::Proof;
use signup_sequencer::identity_tree::{InclusionProof, TreeState, TreeVersionReadOps};
use signup_sequencer::server::data::{
    AddBatchSizeRequest, DeletionRequest, InclusionProofRequest, InclusionProofResponse,
    InsertCommitmentRequest, RecoveryRequest, RemoveBatchSizeRequest, VerifySemaphoreProofRequest,
};
use signup_sequencer::task_monitor::TaskMonitor;
use testcontainers::clients::Cli;
use tracing::trace;

use self::chain_mock::{spawn_mock_chain, MockChain, SpecialisedContract};
use self::prelude::*;
use crate::common::abi::{IWorldIDIdentityManager, RootInfo};
use crate::common::chain_mock::SpecialisedClient;
use crate::server::error::Error as ServerError;

const NUM_ATTEMPTS_FOR_INCLUSION_PROOF: usize = 20;

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
    test_verify_proof_inner(
        uri,
        client,
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
        None,
        expected_failure,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[instrument(skip_all)]
pub async fn test_verify_proof_with_age(
    uri: &str,
    client: &Client<HttpConnector>,
    root: Field,
    signal_hash: Field,
    nullifier_hash: Field,
    external_nullifier_hash: Field,
    proof: protocol::Proof,
    max_root_age_seconds: i64,
    expected_failure: Option<&str>,
) {
    test_verify_proof_inner(
        uri,
        client,
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
        Some(max_root_age_seconds),
        expected_failure,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[instrument(skip_all)]
async fn test_verify_proof_inner(
    uri: &str,
    client: &Client<HttpConnector>,
    root: Field,
    signal_hash: Field,
    nullifier_hash: Field,
    external_nullifier_hash: Field,
    proof: protocol::Proof,
    max_root_age_seconds: Option<i64>,
    expected_failure: Option<&str>,
) {
    let body = construct_verify_proof_body(
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
    );

    let uri = match max_root_age_seconds {
        Some(max_root_age_seconds) => {
            format!("{uri}/verifySemaphoreProof?maxRootAgeSeconds={max_root_age_seconds}")
        }
        None => format!("{uri}/verifySemaphoreProof"),
    };

    let req = Request::builder()
        .method("POST")
        .uri(uri)
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
        assert!(
            result.contains(expected_failure),
            "Result (`{}`) did not contain expected failure (`{}`)",
            result,
            expected_failure
        );
    } else {
        assert!(response.status().is_success());
    }
}

#[allow(clippy::too_many_arguments)]
#[instrument(skip_all)]
pub async fn test_verify_proof_on_chain(
    identity_manager: &IWorldIDIdentityManager<SpecialisedClient>,
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
    identity_manager
        .verify_proof(
            root_tok,
            signal_hash_tok,
            nullifier_hash_tok,
            external_nullifier_hash_tok,
            proof_tok,
        )
        .call()
        .await?;

    Ok(())
}

#[instrument(skip_all)]
pub async fn test_inclusion_proof(
    mock_chain: &MockChain,
    uri: &str,
    client: &Client<HttpConnector>,
    leaf_index: usize,
    ref_tree: &PoseidonTree,
    leaf: &Hash,
    expect_failure: bool,
    offchain_mode_enabled: bool,
) {
    for i in 0..NUM_ATTEMPTS_FOR_INCLUSION_PROOF {
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
        let result = serde_json::from_str::<InclusionProofResponse>(&result)
            .expect("Failed to parse response as json");

        if let Some(root) = result.0.root {
            if offchain_mode_enabled {
                // For offchain mode returning root in inclusion proof response means the proof
                // is valid.
                let proof_json = generate_reference_proof(ref_tree, leaf_index);
                assert_eq!(result, proof_json);

                return;
            }
            let root: U256 = root.into();

            let (root, ..) = mock_chain
                .identity_manager
                .query_root(root)
                .call()
                .await
                .expect("Failed to call method queryRoot on mocked chain.");

            if root != U256::zero() {
                let proof_json = generate_reference_proof(ref_tree, leaf_index);
                assert_eq!(result, proof_json);

                return;
            }
        }

        assert_eq!(result, generate_reference_proof(ref_tree, leaf_index));
        assert_eq!(response.status(), StatusCode::OK);
        info!("Got pending, waiting 5 seconds, iteration {}", i);
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    panic!(
        "Failed to get an inclusion proof after {} attempts!",
        NUM_ATTEMPTS_FOR_INCLUSION_PROOF
    );
}

#[instrument(skip_all)]
pub async fn test_inclusion_proof_mined(
    mock_chain: &MockChain,
    uri: &str,
    client: &Client<HttpConnector>,
    leaf: &Hash,
    expect_failure: bool,
    offchain_mode_enabled: bool,
) {
    for i in 0..NUM_ATTEMPTS_FOR_INCLUSION_PROOF {
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
        let result = serde_json::from_str::<InclusionProofResponse>(&result)
            .expect("Failed to parse response as json");

        if let Some(root) = result.0.root {
            if offchain_mode_enabled {
                // For offchain mode returning root in inclusion proof response means the proof
                // is valid.
                return;
            }
            let root: U256 = root.into();

            let (root, ..) = mock_chain
                .identity_manager
                .query_root(root)
                .call()
                .await
                .expect("Failed to call method queryRoot on mocked chain.");

            if root != U256::zero() {
                return;
            }
        }

        info!("Got pending, waiting 5 seconds, iteration {}", i);
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    panic!(
        "Failed to get an inclusion proof after {} attempts!",
        NUM_ATTEMPTS_FOR_INCLUSION_PROOF
    );
}

#[instrument(skip_all)]
pub async fn test_not_in_tree(uri: &str, client: &Client<HttpConnector>, leaf: &Hash) {
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

    assert_eq!(response.status(), StatusCode::OK);

    let bytes = hyper::body::to_bytes(response.body_mut())
        .await
        .expect("Failed to convert response body to bytes");
    let result = String::from_utf8(bytes.into_iter().collect())
        .expect("Could not parse response bytes to utf-8");

    let result = serde_json::from_str::<InclusionProofResponse>(&result)
        .expect("Failed to parse InclusionProofResponse");

    assert_eq!(result.0.root, None);
}

#[instrument(skip_all)]
pub async fn test_in_tree(uri: &str, client: &Client<HttpConnector>, leaf: &Hash) {
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

    assert_eq!(response.status(), StatusCode::OK);

    let bytes = hyper::body::to_bytes(response.body_mut())
        .await
        .expect("Failed to convert response body to bytes");
    let result = String::from_utf8(bytes.into_iter().collect())
        .expect("Could not parse response bytes to utf-8");

    let result = serde_json::from_str::<InclusionProofResponse>(&result)
        .expect("Failed to parse InclusionProofResponse");

    let root: U256 = result.0.root.expect("Failed to get root").into();

    assert_ne!(root, U256::zero(), "Hash is not zero");
}

pub async fn api_delete_identity(
    uri: &str,
    client: &Client<HttpConnector>,
    leaf: &Field,
    expect_failure: bool,
) {
    let body = construct_delete_identity_body(leaf);

    let req = Request::builder()
        .method("POST")
        .uri(uri.to_owned() + "/deleteIdentity")
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

    if expect_failure {
        assert!(!response.status().is_success());
    } else {
        assert!(response.status().is_success());
        assert!(bytes.is_empty());
    }
}

#[instrument(skip_all)]
pub async fn test_delete_identity(
    uri: &str,
    client: &Client<HttpConnector>,
    ref_tree: &mut PoseidonTree,
    test_leaves: &[Field],
    leaf_index: usize,
    expect_failure: bool,
) -> (merkle_tree::Proof<PoseidonHash>, Field) {
    api_delete_identity(uri, client, &test_leaves[leaf_index], expect_failure).await;
    ref_tree.set(leaf_index, Hash::ZERO);
    (ref_tree.proof(leaf_index).unwrap(), ref_tree.root())
}

#[instrument(skip_all)]
pub async fn test_recover_identity(
    uri: &str,
    client: &Client<HttpConnector>,
    ref_tree: &mut PoseidonTree,
    test_leaves: &[Field],
    previous_leaf_index: usize,
    new_leaf: Field,
    new_leaf_index: usize,
    expect_failure: bool,
) -> (merkle_tree::Proof<PoseidonHash>, Field) {
    let previous_leaf = test_leaves[previous_leaf_index];

    let body = construct_recover_identity_body(&previous_leaf, &new_leaf);

    let req = Request::builder()
        .method("POST")
        .uri(uri.to_owned() + "/recoverIdentity")
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

    if expect_failure {
        assert!(!response.status().is_success());
    } else {
        assert!(response.status().is_success());
        assert!(bytes.is_empty());
    }

    // TODO: Note that recovery order is non-deterministic and therefore we cannot
    // easily keep the ref_tree in sync with the sequencer's version of the
    // tree. In the future, we could consider tracking updates to the tree in a
    // different way like listening to event emission.
    ref_tree.set(previous_leaf_index, Hash::ZERO);
    // Continuing on the note above, while the replacement identity is be
    // inserted as a new identity, it is not deterministic and if there are multiple
    // recovery requests, it is possible that the sequencer tree is ordered in a
    // different way than the ref_tree
    ref_tree.set(new_leaf_index, new_leaf);
    (ref_tree.proof(new_leaf_index).unwrap(), ref_tree.root())
}

#[instrument(skip_all)]
pub async fn test_add_batch_size(
    uri: impl Into<String>,
    prover_url: impl Into<String>,
    batch_size: u64,
    prover_type: ProverType,
    client: &Client<HttpConnector>,
) -> anyhow::Result<()> {
    let body = Body::from(serde_json::to_string(&AddBatchSizeRequest {
        url: prover_url.into(),
        batch_size: batch_size as usize,
        timeout_seconds: 3,
        prover_type,
    })?);
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
    prover_type: ProverType,
    expect_failure: bool,
) -> anyhow::Result<()> {
    let body = Body::from(serde_json::to_string(&RemoveBatchSizeRequest {
        batch_size: batch_size as usize,
        prover_type,
    })?);
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
pub async fn api_insert_identity(uri: &str, client: &Client<HttpConnector>, leaf: &Field) {
    let body = construct_insert_identity_body(leaf);
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
}

#[instrument(skip_all)]
pub async fn test_insert_identity(
    uri: &str,
    client: &Client<HttpConnector>,
    ref_tree: &mut PoseidonTree,
    test_leaves: &[Field],
    leaf_index: usize,
) -> (merkle_tree::Proof<PoseidonHash>, Field) {
    api_insert_identity(uri, client, &test_leaves[leaf_index]).await;

    ref_tree.set(leaf_index, test_leaves[leaf_index]);

    (ref_tree.proof(leaf_index).unwrap(), ref_tree.root())
}

fn construct_inclusion_proof_body(identity_commitment: &Hash) -> Body {
    Body::from(
        serde_json::to_string(&InclusionProofRequest {
            identity_commitment: *identity_commitment,
        })
        .expect("Cannot serialize InclusionProofRequest"),
    )
}

fn construct_delete_identity_body(identity_commitment: &Hash) -> Body {
    Body::from(
        serde_json::to_string(&DeletionRequest {
            identity_commitment: *identity_commitment,
        })
        .expect("Cannot serialize DeletionRequest"),
    )
}

pub fn construct_recover_identity_body(
    previous_identity_commitment: &Hash,
    new_identity_commitment: &Hash,
) -> Body {
    Body::from(
        serde_json::to_string(&RecoveryRequest {
            previous_identity_commitment: *previous_identity_commitment,
            new_identity_commitment:      *new_identity_commitment,
        })
        .expect("Cannot serialize RecoveryRequest"),
    )
}

pub fn construct_insert_identity_body(identity_commitment: &Field) -> Body {
    Body::from(
        serde_json::to_string(&InsertCommitmentRequest {
            identity_commitment: *identity_commitment,
        })
        .expect("Cannot serialize InsertCommitmentRequest"),
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
        serde_json::to_string(&VerifySemaphoreProofRequest {
            root,
            signal_hash,
            nullifier_hash: nullifer_hash,
            external_nullifier_hash,
            proof,
        })
        .expect("Cannot serialize VerifySemaphoreProofRequest"),
    )
}

#[instrument(skip_all)]
#[allow(clippy::type_complexity)]
pub async fn spawn_app(
    config: Config,
) -> anyhow::Result<(Arc<App>, JoinHandle<()>, SocketAddr, Arc<Shutdown>)> {
    let server_config = config.server.clone();
    let app = App::new(config).await.expect("Failed to create App");
    let shutdown = Arc::new(Shutdown::new());

    let task_monitor = TaskMonitor::new(app.clone(), shutdown.clone());
    task_monitor.start().await;

    let listener = TcpListener::bind(server_config.address).expect("Failed to bind random port");
    let local_addr = listener.local_addr()?;

    info!("Waiting for tree initialization");
    // For our tests to work we need the tree to be initialized.
    while app.tree_state().is_err() {
        trace!("Waiting for the tree to be initialized");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    let app_clone = app.clone();
    let shutdown_clone = shutdown.clone();
    let app_handle = spawn({
        async move {
            info!("App thread starting");
            server::bind_from_listener(
                app_clone,
                Duration::from_secs(30),
                listener,
                shutdown_clone,
            )
            .await
            .expect("Failed to bind address");
            info!("App thread stopping");
        }
    });

    info!("Checking app health");
    check_health(&local_addr).await?;

    info!("Checking metrics");
    check_metrics(&local_addr).await?;

    info!("App ready");

    Ok((app, app_handle, local_addr, shutdown))
}

pub async fn check_metrics(socket_addr: &SocketAddr) -> anyhow::Result<()> {
    let uri = format!("http://{}", socket_addr);
    let client = Client::new();
    let req = Request::builder()
        .method("GET")
        .uri(uri.to_owned() + "/metrics")
        .body(Body::empty())
        .expect("Failed to create metrics hyper::Body");
    let response = client
        .request(req)
        .await
        .context("Failed to execute metrics request.")?;
    if !response.status().is_success() {
        anyhow::bail!("Metrics endpoint failed");
    }

    Ok(())
}

pub async fn check_health(socket_addr: &SocketAddr) -> anyhow::Result<()> {
    let uri = format!("http://{}", socket_addr);
    let client = Client::new();
    let req = Request::builder()
        .method("GET")
        .uri(uri.to_owned() + "/health")
        .body(Body::empty())
        .expect("Failed to create health check hyper::Body");
    let response = client
        .request(req)
        .await
        .context("Failed to execute health check request.")?;
    if !response.status().is_success() {
        anyhow::bail!("Health check failed");
    }

    Ok(())
}

#[derive(Deserialize, Serialize, Debug)]
struct CompiledContract {
    abi:      Abi,
    bytecode: Bytecode,
}

pub async fn spawn_deps<'a, 'b, 'c>(
    initial_root: U256,
    insertion_batch_sizes: &'b [usize],
    deletion_batch_sizes: &'c [usize],
    tree_depth: u8,
    docker: &'a Cli,
) -> anyhow::Result<(
    MockChain,
    DockerContainer<'a>,
    HashMap<usize, ProverService>,
    HashMap<usize, ProverService>,
    micro_oz::ServerHandle,
)> {
    let chain = spawn_mock_chain(
        initial_root,
        insertion_batch_sizes,
        deletion_batch_sizes,
        tree_depth,
    );

    let db_container = spawn_db(docker);

    let insertion_prover_futures = FuturesUnordered::new();
    for batch_size in insertion_batch_sizes {
        insertion_prover_futures.push(spawn_mock_insertion_prover(*batch_size, tree_depth));
    }

    let deletion_prover_futures = FuturesUnordered::new();
    for batch_size in deletion_batch_sizes {
        deletion_prover_futures.push(spawn_mock_deletion_prover(*batch_size, tree_depth));
    }

    let (chain, db_container, insertion_provers, deletion_provers) = tokio::join!(
        chain,
        db_container,
        insertion_prover_futures.collect::<Vec<_>>(),
        deletion_prover_futures.collect::<Vec<_>>()
    );

    let chain = chain?;

    let micro_oz = micro_oz::spawn(chain.anvil.endpoint(), chain.private_key.clone()).await?;

    let insertion_provers = insertion_provers
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    let insertion_prover_map = insertion_provers
        .into_iter()
        .map(|prover| (prover.batch_size(), prover))
        .collect::<HashMap<usize, ProverService>>();

    let deletion_provers = deletion_provers
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    let deletion_prover_map = deletion_provers
        .into_iter()
        .map(|prover| (prover.batch_size(), prover))
        .collect::<HashMap<usize, ProverService>>();

    Ok((
        chain,
        db_container?,
        insertion_prover_map,
        deletion_prover_map,
        micro_oz,
    ))
}

async fn spawn_db(docker: &Cli) -> anyhow::Result<DockerContainer> {
    let db_container = postgres_docker_utils::setup(docker).await.unwrap();

    Ok(db_container)
}

pub async fn spawn_mock_insertion_prover(
    batch_size: usize,
    tree_depth: u8,
) -> anyhow::Result<ProverService> {
    let mock_prover_service =
        prover_mock::ProverService::new(batch_size, tree_depth, ProverType::Insertion).await?;

    Ok(mock_prover_service)
}

pub async fn spawn_mock_deletion_prover(
    batch_size: usize,
    tree_depth: u8,
) -> anyhow::Result<ProverService> {
    let mock_prover_service =
        prover_mock::ProverService::new(batch_size, tree_depth, ProverType::Deletion).await?;

    Ok(mock_prover_service)
}

/// Initializes the tracing subscriber.
///
/// Set the `QUIET_MODE` environment variable to reduce the complexity of the
/// log output.
pub fn init_tracing_subscriber() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let quiet_mode = std::env::var("QUIET_MODE").is_ok();
        let result = if quiet_mode {
            tracing_subscriber::fmt()
                .with_env_filter("info,signup_sequencer=debug")
                .compact()
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
    });
}

pub fn generate_reference_proof(
    ref_tree: &PoseidonTree,
    leaf_idx: usize,
) -> InclusionProofResponse {
    InclusionProofResponse(InclusionProof {
        root:    Some(ref_tree.root()),
        proof:   ref_tree.proof(leaf_idx),
        message: None,
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

#[instrument(skip_all)]
pub async fn test_same_tree_states(
    tree_state1: &TreeState,
    tree_state2: &TreeState,
) -> anyhow::Result<()> {
    assert_eq!(
        tree_state1.processed_tree().next_leaf(),
        tree_state2.processed_tree().next_leaf()
    );
    assert_eq!(
        tree_state1.processed_tree().get_root(),
        tree_state2.processed_tree().get_root()
    );

    assert_eq!(
        tree_state1.batching_tree().next_leaf(),
        tree_state2.batching_tree().next_leaf()
    );
    assert_eq!(
        tree_state1.batching_tree().get_root(),
        tree_state2.batching_tree().get_root()
    );

    assert_eq!(
        tree_state1.latest_tree().next_leaf(),
        tree_state2.latest_tree().next_leaf()
    );
    assert_eq!(
        tree_state1.latest_tree().get_root(),
        tree_state2.latest_tree().get_root()
    );

    Ok(())
}
