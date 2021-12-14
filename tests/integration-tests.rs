use ethers::{
    abi::Address,
    core::abi::Abi,
    prelude::{
        Bytes, ContractFactory, Http, LocalWallet, NonceManagerMiddleware, Provider, Signer,
        SignerMiddleware,
    },
    utils::{Ganache, GanacheInstance},
};
use eyre::{bail, Result as EyreResult};
use hex_literal::hex;
use hyper::{client::HttpConnector, Body, Client, Request};
use serde::{Deserialize, Serialize};
use serde_json::json;
use signup_sequencer::{
    app::App,
    hash::Hash,
    mimc_tree::MimcTree,
    server::{self, InclusionProofRequest},
    Options,
};
use std::{
    fs::File,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use structopt::StructOpt;
use tempfile::NamedTempFile;
use tokio::{spawn, sync::broadcast};
use url::{Host, Url};

const TEST_LEAFS: &[&str] = &[
    "0000000000000000000000000000000000000000000000000000000000000001",
    "0000000000000000000000000000000000000000000000000000000000000002",
];

const GANACHE_DEFAULT_WALLET_KEY: Hash = Hash(hex!(
    "1ce6a4cc4c9941a4781349f988e129accdc35a55bb3d5b1a7b342bc2171db484"
));

#[tokio::test]
async fn insert_identity_and_proofs() {
    let mut options = Options::from_iter_safe(&[""]).expect("Failed to create options");
    options.server.server = Url::parse("http://127.0.0.1:0/").expect("Failed to parse URL");

    let temp_commitments_file = NamedTempFile::new().expect("Failed to create named temp file");
    options.app.storage_file = temp_commitments_file.path().to_path_buf();

    let (shutdown, _) = broadcast::channel(1);

    let (ganache, semaphore_address) = spawn_mock_chain()
        .await
        .expect("Failed to spawn ganache chain");

    options.app.ethereum.eip1559 = false;
    options.app.ethereum.ethereum_provider =
        Url::parse(&ganache.endpoint()).expect("Failed to parse ganache endpoint");
    options.app.ethereum.semaphore_address = semaphore_address;
    options.app.ethereum.signing_key = GANACHE_DEFAULT_WALLET_KEY;

    let local_addr = spawn_app(options.clone(), shutdown.clone())
        .await
        .expect("Failed to spawn app.");

    let uri = "http://".to_owned() + &local_addr.to_string();
    let mut ref_tree = MimcTree::new(options.app.tree_depth, options.app.initial_leaf);
    let client = Client::new();

    test_inclusion_proof(&uri, &client, 0, &mut ref_tree, &options.app.initial_leaf).await;
    test_inclusion_proof(&uri, &client, 1, &mut ref_tree, &options.app.initial_leaf).await;
    test_insert_identity(&uri, &client, TEST_LEAFS[0], 0).await;
    test_inclusion_proof(
        &uri,
        &client,
        0,
        &mut ref_tree,
        &Hash::from_str(TEST_LEAFS[0]).expect("Failed to parse Hash from test leaf 0"),
    )
    .await;
    test_insert_identity(&uri, &client, TEST_LEAFS[1], 1).await;
    test_inclusion_proof(
        &uri,
        &client,
        1,
        &mut ref_tree,
        &Hash::from_str(TEST_LEAFS[1]).expect("Failed to parse Hash from test leaf 1"),
    )
    .await;
    test_inclusion_proof(&uri, &client, 2, &mut ref_tree, &options.app.initial_leaf).await;

    // Shutdown app and spawn new one from file
    let _ = shutdown.send(()).expect("Failed to send shutdown signal");

    let local_addr = spawn_app(options.clone(), shutdown.clone())
        .await
        .expect("Failed to spawn app.");
    let uri = "http://".to_owned() + &local_addr.to_string();

    test_inclusion_proof(
        &uri,
        &client,
        0,
        &mut ref_tree,
        &Hash::from_str(TEST_LEAFS[0]).expect("Failed to parse Hash from test leaf 0"),
    )
    .await;
    test_inclusion_proof(
        &uri,
        &client,
        1,
        &mut ref_tree,
        &Hash::from_str(TEST_LEAFS[1]).expect("Failed to parse Hash from test leaf 1"),
    )
    .await;

    temp_commitments_file
        .close()
        .expect("Failed to close temp file");
}

async fn test_inclusion_proof(
    uri: &str,
    client: &Client<HttpConnector>,
    leaf_index: usize,
    ref_tree: &mut MimcTree,
    leaf: &Hash,
) {
    let body = construct_inclusion_proof_body(leaf_index);
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
    assert!(response.status().is_success());

    let bytes = hyper::body::to_bytes(response.body_mut())
        .await
        .expect("Failed to convert response body to bytes");
    let result = String::from_utf8(bytes.into_iter().collect())
        .expect("Could not parse response bytes to utf-8");

    ref_tree.set(leaf_index, *leaf);
    let proof = ref_tree.proof(leaf_index).expect("Ref tree malfunctioning");
    let serialized_proof =
        serde_json::to_string_pretty(&proof).expect("Proof serialization failed");

    assert_eq!(result, serialized_proof);
}

async fn test_insert_identity(
    uri: &str,
    client: &Client<HttpConnector>,
    identity_commitment: &str,
    identity_index: usize,
) {
    let body = construct_insert_identity_body(identity_commitment);
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
    assert!(response.status().is_success());

    let bytes = hyper::body::to_bytes(response.body_mut())
        .await
        .expect("Failed to convert response body to bytes");
    let result = String::from_utf8(bytes.into_iter().collect())
        .expect("Could not parse response bytes to utf-8");

    let expected = InclusionProofRequest { identity_index };
    let expected = serde_json::to_string_pretty(&expected).expect("Index serialization failed");

    assert_eq!(result, expected);
}

fn construct_inclusion_proof_body(identity_index: usize) -> Body {
    Body::from(
        json!({
            "identityIndex": identity_index,
        })
        .to_string(),
    )
}

fn construct_insert_identity_body(identity_commitment: &str) -> Body {
    Body::from(
        json!({
            "identityCommitment": identity_commitment,

        })
        .to_string(),
    )
}

async fn spawn_app(options: Options, shutdown: broadcast::Sender<()>) -> EyreResult<SocketAddr> {
    let app = Arc::new(App::new(options.app).await.expect("Failed to create App"));

    let ip: IpAddr = match options.server.server.host() {
        Some(Host::Ipv4(ip)) => ip.into(),
        Some(Host::Ipv6(ip)) => ip.into(),
        Some(_) => bail!("Cannot bind {}", options.server.server),
        None => Ipv4Addr::LOCALHOST.into(),
    };
    let port = options.server.server.port().unwrap_or(9998);
    let addr = SocketAddr::new(ip, port);
    let listener = TcpListener::bind(&addr).expect("Failed to bind random port");
    let local_addr = listener.local_addr()?;

    spawn({
        async move {
            server::bind_from_listener(app, listener, shutdown)
                .await
                .expect("Failed to bind address");
        }
    });

    Ok(local_addr)
}

#[derive(Deserialize, Serialize, Debug)]
struct CompiledContract {
    abi:      Abi,
    bytecode: String,
}

fn deserialize_to_bytes(input: String) -> EyreResult<Bytes> {
    if input.len() >= 2 && &input[0..2] == "0x" {
        let bytes: Vec<u8> = hex::decode(&input[2..])?;
        Ok(bytes.into())
    } else {
        bail!("Expected 0x prefix")
    }
}

async fn spawn_mock_chain() -> EyreResult<(GanacheInstance, Address)> {
    let ganache = Ganache::new().block_time(2u64).mnemonic("test").spawn();

    let provider = Provider::<Http>::try_from(ganache.endpoint())
        .expect("Failed to initialize ganache endpoint")
        .interval(Duration::from_millis(500u64));

    let wallet: LocalWallet = ganache.keys()[0].clone().into();

    // connect the wallet to the provider
    let client = SignerMiddleware::new(provider, wallet.clone());
    let client = NonceManagerMiddleware::new(client, wallet.address());
    let client = std::sync::Arc::new(client);

    let mimc_json = File::open("./sol/MiMC.json").expect("Failed to read MiMC.sol");
    let mimc_json: CompiledContract =
        serde_json::from_reader(mimc_json).expect("Could not parse compiled MiMC contract");
    let mimc_bytecode = deserialize_to_bytes(mimc_json.bytecode)?;

    let mimc_factory = ContractFactory::new(mimc_json.abi, mimc_bytecode, client.clone());

    let mimc_contract = mimc_factory
        .deploy(())?
        .legacy()
        .confirmations(0usize)
        .send()
        .await?;

    let semaphore_json =
        File::open("./sol/Semaphore.json").expect("Compiled contract doesn't exist");
    let semaphore_json: CompiledContract =
        serde_json::from_reader(semaphore_json).expect("Could not read contract");

    let semaphore_bytecode = semaphore_json.bytecode.replace(
        "__$cf5da3090e28b1d67a537682696360513a$__",
        &format!("{:?}", mimc_contract.address()).replace("0x", ""),
    );
    let semaphore_bytecode = deserialize_to_bytes(semaphore_bytecode)?;

    // create a factory which will be used to deploy instances of the contract
    let semaphore_factory =
        ContractFactory::new(semaphore_json.abi, semaphore_bytecode, client.clone());

    let semaphore_contract = semaphore_factory
        .deploy((4_u64, 123_u64))?
        .legacy()
        .confirmations(0usize)
        .send()
        .await?;

    Ok((ganache, semaphore_contract.address()))
}
