mod abi;
mod prover_mock;
use std::{
    fs::File,
    io::BufReader,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    sync::Arc,
    time::Duration,
};

use clap::Parser;
use cli_batteries::{reset_shutdown, shutdown};
use ethers::{
    abi::{AbiEncode, Address},
    core::{abi::Abi, k256::ecdsa::SigningKey, rand},
    prelude::{
        artifacts::Bytecode, ContractFactory, Http, LocalWallet, NonceManagerMiddleware, Provider,
        Signer, SignerMiddleware, Wallet,
    },
    providers::Middleware,
    types::{BlockNumber, Bytes, Filter, Log, H160, H256, U256},
    utils::{Anvil, AnvilInstance},
};
use hyper::{client::HttpConnector, Body, Client, Request};
use once_cell::sync::Lazy;
use semaphore::{
    hash_to_field,
    identity::Identity,
    merkle_tree::{self, Branch},
    poseidon_tree::{PoseidonHash, PoseidonTree},
    protocol::{self, generate_nullifier_hash, generate_proof, verify_proof},
    Field,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tempfile::tempdir;
use tokio::{spawn, task::JoinHandle};
use tracing::{error, info, instrument};
use tracing_subscriber::fmt::{format::FmtSpan, time::Uptime};
use url::{Host, Url};

use abi as ContractAbi;
use signup_sequencer::{app::App, identity_tree::Hash, server, Options};

#[tokio::test]
#[serial_test::serial]
async fn validate_proofs() {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let db_dir = tempdir().unwrap();
    let db = db_dir.path().join("test.db");

    let mut options = Options::try_parse_from([
        "signup-sequencer",
        "--identity-manager-address",
        "0x0000000000000000000000000000000000000000", // placeholder, updated below
        "--database",
        &format!("sqlite://{}", db.to_str().unwrap()),
        "--database-max-connections",
        "1",
        "--tree-depth",
        "20",
    ])
    .expect("Failed to create options");
    options.server.server = Url::parse("http://127.0.0.1:0/").expect("Failed to parse URL");

    let mut ref_tree = PoseidonTree::new(21, options.app.contracts.initial_leaf_value);
    let initial_root: U256 = ref_tree.root().into();
    let (chain, private_key, identity_manager_address, prover_mock) =
        spawn_mock_chain(initial_root)
            .await
            .expect("Failed to spawn mock chain");

    options.app.contracts.identity_manager_address = identity_manager_address;
    options.app.ethereum.read_options.confirmation_blocks_delay = 2;
    options.app.ethereum.read_options.ethereum_provider =
        Url::parse(&chain.endpoint()).expect("Failed to parse ganache endpoint");
    options.app.ethereum.write_options.signing_key = private_key;

    let (app, local_addr) = spawn_app(options.clone())
        .await
        .expect("Failed to spawn app.");

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    static IDENTITIES: Lazy<Vec<Identity>> = Lazy::new(|| {
        vec![
            Identity::from_seed(b"test_f0f0"),
            Identity::from_seed(b"test_f1f1"),
            Identity::from_seed(b"test_f2f2"),
        ]
    });
    
    const TEST_LEAVES: Lazy<Vec<Field>> =
        Lazy::new(|| IDENTITIES.iter().map(|id| id.commitment()).collect());
    
    
    // generate identity
    let (merkle_proof, root) =
        test_insert_identity(&uri, &client, &mut ref_tree, &TEST_LEAVES, 0).await;

    // simulate client generating a proof
    let signal_hash = hash_to_field(b"signal_hash");
    let external_nullifier_hash = hash_to_field(b"external_hash");
    let nullifier_hash = generate_nullifier_hash(&IDENTITIES[0], external_nullifier_hash);

    let proof = generate_proof(
        &IDENTITIES[0],
        &merkle_proof,
        external_nullifier_hash,
        signal_hash,
    )
    .unwrap();

    test_verify_proof(
        &uri,
        &client,
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
        false,
    )
    .await;

    // Shutdown the app properly for the final time
    shutdown();
    app.await.unwrap();
    prover_mock.stop();
    reset_shutdown();
}

#[tokio::test]
#[serial_test::serial]
async fn insert_identity_and_proofs() {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let db_dir = tempdir().unwrap();
    let db = db_dir.path().join("test.db");
    let batch_size: usize = 3;

    let mut options = Options::try_parse_from([
        "signup-sequencer",
        "--identity-manager-address",
        "0x0000000000000000000000000000000000000000", // placeholder, updated below
        "--database",
        &format!("sqlite://{}", db.to_str().unwrap()),
        "--database-max-connections",
        "1",
        "--tree-depth",
        "20",
        "--batch-size",
        &format!("{batch_size}"),
        "--batch-timeout-seconds",
        "10",
    ])
    .expect("Failed to create options");
    options.server.server = Url::parse("http://127.0.0.1:0/").expect("Failed to parse URL");

    let mut ref_tree = PoseidonTree::new(21, options.app.contracts.initial_leaf_value);
    let initial_root: U256 = ref_tree.root().into();
    let (chain, private_key, identity_manager_address, prover_mock) =
        spawn_mock_chain(initial_root)
            .await
            .expect("Failed to spawn mock chain");

    options.app.contracts.identity_manager_address = identity_manager_address;
    options.app.ethereum.read_options.confirmation_blocks_delay = 2;
    options.app.ethereum.read_options.ethereum_provider =
        Url::parse(&chain.endpoint()).expect("Failed to parse ganache endpoint");
    options.app.ethereum.write_options.signing_key = private_key;

    let (app, local_addr) = spawn_app(options.clone())
        .await
        .expect("Failed to spawn app.");

    let test_identities = generate_test_identities(batch_size * 3);
    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    // Check that we can get inclusion proofs for things that already exist in the
    // database and on chain.
    test_inclusion_proof(
        &uri,
        &client,
        0,
        &mut ref_tree,
        &options.app.contracts.initial_leaf_value,
        true,
    )
    .await;
    test_inclusion_proof(
        &uri,
        &client,
        1,
        &mut ref_tree,
        &options.app.contracts.initial_leaf_value,
        true,
    )
    .await;

    // Insert enough identities to trigger an batch to be sent to the blockchain
    // based on the current batch size of 3.
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 0).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 1).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 2).await;

    // Check that we can get their inclusion proofs back.
    test_inclusion_proof(
        &uri,
        &client,
        0,
        &mut ref_tree,
        &Hash::from_str_radix(&test_identities[0], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
    )
    .await;
    test_inclusion_proof(
        &uri,
        &client,
        1,
        &mut ref_tree,
        &Hash::from_str_radix(&test_identities[1], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
    )
    .await;
    test_inclusion_proof(
        &uri,
        &client,
        2,
        &mut ref_tree,
        &Hash::from_str_radix(&test_identities[2], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
    )
    .await;

    // Insert too few identities to trigger a batch, and then force the timeout to
    // complete and submit a partial batch to the chain.
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 3).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 4).await;
    tokio::time::pause();
    tokio::time::resume();

    // Check that we can also get these inclusion proofs back.
    test_inclusion_proof(
        &uri,
        &client,
        3,
        &mut ref_tree,
        &Hash::from_str_radix(&test_identities[3], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
    )
    .await;
    test_inclusion_proof(
        &uri,
        &client,
        4,
        &mut ref_tree,
        &Hash::from_str_radix(&test_identities[4], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
    )
    .await;

    // Shutdown the app and reset the mock shutdown, allowing us to test the
    // behaviour with saved data.
    info!("Stopping the app for testing purposes");
    shutdown();
    app.await.unwrap();
    reset_shutdown();

    // Test loading the state from a file when the on-chain contract has the state.
    let (app, local_addr) = spawn_app(options.clone())
        .await
        .expect("Failed to spawn app.");
    let uri = "http://".to_owned() + &local_addr.to_string();

    // Check that we can still get inclusion proofs for identities we know to have
    // been inserted previously. Here we check the first and last ones we inserted.
    test_inclusion_proof(
        &uri,
        &client,
        0,
        &mut ref_tree,
        &Hash::from_str_radix(&test_identities[0], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
    )
    .await;
    test_inclusion_proof(
        &uri,
        &client,
        4,
        &mut ref_tree,
        &Hash::from_str_radix(&test_identities[4], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
    )
    .await;

    // Shutdown the app and reset the mock shutdown, allowing us to test the
    // behaviour with the saved tree.
    info!("Stopping the app for testing purposes");
    shutdown();
    app.await.unwrap();
    reset_shutdown();

    // Test loading the state from the saved tree when the on-chain contract has the
    // state.
    let (app, local_addr) = spawn_app(options.clone())
        .await
        .expect("Failed to spawn app.");
    let uri = "http://".to_owned() + &local_addr.to_string();

    // Check that we can still get inclusion proofs for identities we know to have
    // been inserted previously. Here we check the first and last ones we inserted.
    test_inclusion_proof(
        &uri,
        &client,
        0,
        &mut ref_tree,
        &Hash::from_str_radix(&test_identities[0], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
    )
    .await;
    test_inclusion_proof(
        &uri,
        &client,
        4,
        &mut ref_tree,
        &Hash::from_str_radix(&test_identities[4], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
    )
    .await;

    // Shutdown the app properly for the final time
    shutdown();
    app.await.unwrap();
    prover_mock.stop();
    reset_shutdown();
}

#[instrument(skip_all)]
async fn wait_for_log_count(
    provider: &Provider<Http>,
    identity_manager_address: H160,
    expected_count: usize,
) {
    for i in 1..21 {
        let filter = Filter::new()
            .address(identity_manager_address)
            .from_block(BlockNumber::Earliest)
            .to_block(BlockNumber::Latest);
        let result: Vec<Log> = provider.request("eth_getLogs", [filter]).await.unwrap();

        if result.len() >= expected_count {
            info!(
                "Got {} logs (vs expected {}), done in iteration {}: {:?}",
                result.len(),
                expected_count,
                i,
                result
            );

            // TODO: Figure out a better way to do this.
            // Getting a log event is not enough. The app waits for 1 transaction
            // confirmation. It will arrive only after the first poll interval.
            // The DEFAULT_POLL_INTERVAL in ethers-providers is 7 seconds.
            tokio::time::sleep(Duration::from_secs(8)).await;

            return;
        }

        info!(
            "Got {} logs (vs expected {}), waiting 1 second, iteration {}",
            result.len(),
            expected_count,
            i
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    panic!("Failed waiting for {expected_count} log events");
}

#[instrument(skip_all)]
async fn test_verify_proof(
    uri: &str,
    client: &Client<HttpConnector>,
    root: Field,
    signal_hash: Field,
    nullifer_hash: Field,
    external_nullifier_hash: Field,
    proof: protocol::Proof,
    expect_failure: bool,
) {
    let body = construct_verify_proof_body(
        root,
        signal_hash,
        nullifer_hash,
        external_nullifier_hash,
        proof,
    );
    let req = Request::builder()
        .method("POST")
        .uri(uri.to_owned() + "/verifyProof")
        .header("Content-Type", "application/json")
        .body(body)
        .expect("Failed to create verify proof hyper::Body");
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
}

#[instrument(skip_all)]
async fn test_inclusion_proof(
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
async fn test_insert_identity(
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
    let body = Body::from(
        json!({
            "identityCommitment": identity_commitment,
        })
        .to_string(),
    );
    println!("body {:?}", body);
    body
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
async fn spawn_app(options: Options) -> anyhow::Result<(JoinHandle<()>, SocketAddr)> {
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

#[instrument(skip_all)]
async fn spawn_mock_chain(
    initial_root: U256,
) -> anyhow::Result<(AnvilInstance, H256, Address, prover_mock::Service)> {
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

    // Load the contracts to push to the mock chain
    let mock_state_bridge_factory =
        load_and_build_contract("./sol/SimpleStateBridge.json", client.clone())?;
    let mock_state_bridge = mock_state_bridge_factory
        .deploy(())?
        .confirmations(0usize)
        .send()
        .await?;

    let mock_verifier_factory =
        load_and_build_contract("./sol/SimpleVerifier.json", client.clone())?;
    let mock_verifier = mock_verifier_factory
        .deploy(())?
        .confirmations(0usize)
        .send()
        .await?;

    let identity_manager_impl_factory =
        load_and_build_contract("./sol/WorldIDIdentityManagerImplV1.json", client.clone())?;
    let identity_manager_impl = identity_manager_impl_factory
        .deploy(())?
        .confirmations(0usize)
        .send()
        .await?;

    let identity_manager_factory =
        load_and_build_contract("./sol/WorldIDIdentityManager.json", client.clone())?;
    let state_bridge_address = mock_state_bridge.address();
    let verifier_address = mock_verifier.address();
    let enable_state_bridge = true;
    let identity_manager_impl_address = identity_manager_impl.address();
    let init_call_data = ContractAbi::InitializeCall {
        initial_root,
        merkle_tree_verifier: verifier_address,
        enable_state_bridge,
        initial_state_bridge_proxy_address: state_bridge_address,
    };
    let init_call_encoded: Bytes = Bytes::from(init_call_data.encode());

    let identity_manager_contract = identity_manager_factory
        .deploy((identity_manager_impl_address, init_call_encoded))?
        .confirmations(0usize)
        .send()
        .await?;

    let mock_url: String = "0.0.0.0:3001".into();
    let mock_prover_service = prover_mock::Service::new(mock_url).await?;

    Ok((
        chain,
        private_key,
        identity_manager_contract.address(),
        mock_prover_service,
    ))
}

type SharableClient =
    Arc<NonceManagerMiddleware<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>>;
type SpecialisedFactory =
    ContractFactory<NonceManagerMiddleware<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>>;

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
fn init_tracing_subscriber() {
    let quiet_mode = std::env::var("QUIET_MODE").is_ok();
    let result = if quiet_mode {
        tracing_subscriber::fmt()
            .with_env_filter("warn,signup_sequencer=debug")
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

fn generate_reference_proof_json(
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
fn generate_test_identities(identity_count: usize) -> Vec<String> {
    let prefix_regex = regex::Regex::new(r"^0x").unwrap();
    let mut identities = vec![];
    for _ in 0..identity_count {
        // Generate the identities using the just the last 64 bits (of 256) has so we're
        // guaranteed to be less than SNARK_SCALAR_FIELD.
        let bytes: [u8; 32] = U256::from(rand::random::<u64>()).into();
        let identity_string: String = bytes.encode_hex();
        let no_prefix = prefix_regex.replace(&identity_string, "").to_string();

        identities.push(no_prefix);
    }

    identities
}
