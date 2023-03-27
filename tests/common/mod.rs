#![cfg(not(feature = "oz-provider"))]
// We include this module in multiple in multiple integration
// test crates - so some code may not be used in some cases
#![allow(dead_code)]

pub mod abi;
mod prover_mock;

pub mod prelude {
    pub use std::time::Duration;

    pub use anyhow::Context;
    pub use clap::Parser;
    pub use cli_batteries::{reset_shutdown, shutdown};
    pub use ethers::{
        abi::{AbiEncode, Address},
        core::{abi::Abi, k256::ecdsa::SigningKey, rand},
        prelude::{
            artifacts::{Bytecode, BytecodeObject},
            ContractFactory, Http, LocalWallet, NonceManagerMiddleware, Provider, Signer,
            SignerMiddleware, Wallet,
        },
        providers::Middleware,
        types::{Bytes, H256, U256},
        utils::{Anvil, AnvilInstance},
    };
    pub use hyper::{client::HttpConnector, Body, Client, Request};
    pub use once_cell::sync::Lazy;
    pub use postgres_docker_utils::DockerContainerGuard;
    pub use semaphore::{
        hash_to_field,
        identity::Identity,
        merkle_tree::{self, Branch},
        poseidon_tree::{PoseidonHash, PoseidonTree},
        protocol::{self, generate_nullifier_hash, generate_proof},
        Field,
    };
    pub use serde::{Deserialize, Serialize};
    pub use serde_json::json;
    pub use signup_sequencer::{app::App, identity_tree::Hash, server, Options};
    pub use tokio::{spawn, task::JoinHandle};
    pub use tracing::{error, info, instrument};
    pub use tracing_subscriber::fmt::{format::FmtSpan, time::Uptime};
    pub use url::{Host, Url};

    pub use super::{
        abi as ContractAbi, generate_reference_proof_json, generate_test_identities,
        init_tracing_subscriber, prover_mock::ProverService, spawn_app, spawn_deps,
        test_inclusion_proof, test_insert_identity, test_verify_proof, test_verify_proof_on_chain,
    };
}

use ethers::contract::Contract;
use std::{
    fs::File,
    io::BufReader,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    sync::Arc,
};

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
    ref_tree: &mut PoseidonTree,
    leaf: &Hash,
    expect_failure: bool,
) {
    let mut mined_json = None;
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
            info!("Got pending, waiting 1 second, iteration {}", i);
            tokio::time::sleep(Duration::from_secs(1)).await;
        } else {
            mined_json = Some(result_json);
            break;
        }
    }

    let result_json = mined_json.expect("Failed to get mined response");
    let proof_json = generate_reference_proof_json(ref_tree, leaf_index, "mined");
    assert_eq!(result_json, proof_json);
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
    let result = String::from_utf8(bytes.into_iter().collect())
        .expect("Could not parse response bytes to utf-8");
    if !response.status().is_success() {
        panic!("Failed to insert identity: {result}");
    }
    let result_json = serde_json::from_str::<serde_json::Value>(&result)
        .expect("Failed to parse response as json");

    ref_tree.set(leaf_index, test_leaves[leaf_index]);

    let expected_json = generate_reference_proof_json(ref_tree, leaf_index, "pending");

    assert_eq!(result_json, expected_json);

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
    batch_size: usize,
    tree_depth: u8,
) -> anyhow::Result<(MockChain, DockerContainerGuard, ProverService)> {
    let chain = spawn_mock_chain(initial_root, batch_size, tree_depth);
    let db_container = spawn_db();
    let prover = spawn_mock_prover();

    let (chain, db_container, prover) = tokio::join!(chain, db_container, prover);

    Ok((chain?, db_container?, prover?))
}

async fn spawn_db() -> anyhow::Result<DockerContainerGuard> {
    let db_container = postgres_docker_utils::setup().await.unwrap();

    Ok(db_container)
}

pub struct MockChain {
    pub anvil:            AnvilInstance,
    pub private_key:      H256,
    pub identity_manager: SpecialisedContract,
}

#[instrument(skip_all)]
async fn spawn_mock_chain(
    initial_root: U256,
    batch_size: usize,
    tree_depth: u8,
) -> anyhow::Result<MockChain> {
    let chain = Anvil::new().block_time(2u64).spawn();
    let private_key = H256::from_slice(&chain.keys()[0].to_be_bytes());

    let provider = Provider::<Http>::try_from(chain.endpoint())
        .expect("Failed to initialize chain endpoint")
        .interval(Duration::from_millis(500u64));

    let chain_id = provider.get_chainid().await?.as_u64();

    let wallet = LocalWallet::from(chain.keys()[0].clone()).with_chain_id(chain_id);

    // connect the wallet to the provider
    let client = SignerMiddleware::new(provider, wallet.clone());
    let client = NonceManagerMiddleware::new(client, wallet.address());
    let client = Arc::new(client);

    // Loading the semaphore verifier contract is special as it requires replacing
    // the address of the Pairing library.
    let pairing_library_factory = load_and_build_contract("./sol/Pairing.json", client.clone())?;
    let pairing_library = pairing_library_factory
        .deploy(())?
        .confirmations(0usize)
        .send()
        .await?;

    let verifier_path = "./sol/SemaphoreVerifier.json";
    let verifier_file =
        File::open(verifier_path).unwrap_or_else(|_| panic!("Failed to open `{verifier_path}`"));
    let verifier_contract_json: CompiledContract =
        serde_json::from_reader(BufReader::new(verifier_file))
            .unwrap_or_else(|_| panic!("Could not parse the compiled contract at {verifier_path}"));
    let mut verifier_bytecode_object: BytecodeObject = verifier_contract_json.bytecode.object;
    verifier_bytecode_object
        .link_fully_qualified(
            "lib/semaphore/packages/contracts/contracts/base/Pairing.sol:Pairing",
            pairing_library.address(),
        )
        .resolve()
        .unwrap();
    if verifier_bytecode_object.is_unlinked() {
        panic!("Could not link the Pairing library into the Verifier.");
    }

    let bytecode_bytes = verifier_bytecode_object.as_bytes().unwrap_or_else(|| {
        panic!("Could not parse the bytecode for the contract at {verifier_path}")
    });
    let verifier_factory = ContractFactory::new(
        verifier_contract_json.abi,
        bytecode_bytes.clone(),
        client.clone(),
    );
    let semaphore_verifier = verifier_factory.deploy(())?.confirmations(0usize).send();

    // The rest of the contracts can be deployed to the mock chain normally.
    let mock_state_bridge_factory =
        load_and_build_contract("./sol/SimpleStateBridge.json", client.clone())?;
    let mock_state_bridge = mock_state_bridge_factory
        .deploy(())?
        .confirmations(0usize)
        .send();

    let mock_verifier_factory =
        load_and_build_contract("./sol/SequencerVerifier.json", client.clone())?;
    let mock_verifier = mock_verifier_factory
        .deploy(())?
        .confirmations(0usize)
        .send();

    let unimplemented_verifier_factory =
        load_and_build_contract("./sol/UnimplementedTreeVerifier.json", client.clone())?;
    let unimplemented_verifier = unimplemented_verifier_factory
        .deploy(())?
        .confirmations(0usize)
        .send();

    let (semaphore_verifier, mock_state_bridge, mock_verifier, unimplemented_verifier) = tokio::join!(
        semaphore_verifier,
        mock_state_bridge,
        mock_verifier,
        unimplemented_verifier
    );

    let semaphore_verifier = semaphore_verifier?;
    let mock_state_bridge = mock_state_bridge?;
    let mock_verifier = mock_verifier?;
    let unimplemented_verifier = unimplemented_verifier?;

    let verifier_lookup_table_factory =
        load_and_build_contract("./sol/VerifierLookupTable.json", client.clone())?;
    let insert_verifiers = verifier_lookup_table_factory
        .clone()
        .deploy((batch_size as u64, mock_verifier.address()))?
        .confirmations(0usize)
        .send();

    let update_verifiers = verifier_lookup_table_factory
        .deploy((batch_size as u64, unimplemented_verifier.address()))?
        .confirmations(0usize)
        .send();

    let identity_manager_impl_factory =
        load_and_build_contract("./sol/WorldIDIdentityManagerImplV1.json", client.clone())?;
    let identity_manager_impl = identity_manager_impl_factory
        .deploy(())?
        .confirmations(0usize)
        .send();

    let (insert_verifiers, update_verifiers, identity_manager_impl) =
        tokio::join!(insert_verifiers, update_verifiers, identity_manager_impl);

    let insert_verifiers = insert_verifiers?;
    let update_verifiers = update_verifiers?;
    let identity_manager_impl = identity_manager_impl?;

    let identity_manager_factory =
        load_and_build_contract("./sol/WorldIDIdentityManager.json", client.clone())?;
    let state_bridge_address = mock_state_bridge.address();
    let enable_state_bridge = true;
    let identity_manager_impl_address = identity_manager_impl.address();
    let init_call_data = ContractAbi::InitializeCall {
        tree_depth,
        initial_root,
        batch_insertion_verifiers: insert_verifiers.address(),
        batch_update_verifiers: update_verifiers.address(),
        semaphore_verifier: semaphore_verifier.address(),
        enable_state_bridge,
        state_bridge: state_bridge_address,
    };
    let init_call_encoded: Bytes = Bytes::from(init_call_data.encode());

    let identity_manager_contract = identity_manager_factory
        .deploy((identity_manager_impl_address, init_call_encoded))?
        .confirmations(0usize)
        .send()
        .await?;

    let identity_manager: SpecialisedContract = Contract::new(
        identity_manager_contract.address(),
        ContractAbi::BATCHINGCONTRACT_ABI.clone(),
        client.clone(),
    );

    Ok(MockChain {
        anvil: chain,
        private_key,
        identity_manager,
    })
}

async fn spawn_mock_prover() -> anyhow::Result<ProverService> {
    let mock_prover_service = prover_mock::ProverService::new().await?;

    Ok(mock_prover_service)
}

type SpecialisedClient =
    NonceManagerMiddleware<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>;
type SharableClient = Arc<SpecialisedClient>;
type SpecialisedFactory = ContractFactory<SpecialisedClient>;
type SpecialisedContract = Contract<SpecialisedClient>;

fn load_and_build_contract(
    path: impl Into<String>,
    client: SharableClient,
) -> anyhow::Result<SpecialisedFactory> {
    let path_string = path.into();
    let contract_file = File::open(&path_string)
        .unwrap_or_else(|_| panic!("Failed to open `{pth}`", pth = &path_string));

    let contract_json: CompiledContract = serde_json::from_reader(BufReader::new(contract_file))
        .unwrap_or_else(|_| {
            panic!(
                "Could not parse the compiled contract at {pth}",
                pth = &path_string
            )
        });
    let contract_bytecode = contract_json.bytecode.object.as_bytes().unwrap_or_else(|| {
        panic!(
            "Could not parse the bytecode for the contract at {pth}",
            pth = &path_string
        )
    });
    let contract_factory =
        ContractFactory::new(contract_json.abi, contract_bytecode.clone(), client);
    Ok(contract_factory)
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
            .pretty()
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
