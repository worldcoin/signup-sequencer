use eyre::{bail, Result as EyreResult};
use hyper::{client::HttpConnector, Body, Client, Request};
use rust_app_template::{
    app::App,
    hash::Hash,
    mimc_tree::MimcTree,
    server::{self, InclusionProofRequest},
    Options,
};
use serde_json::json;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    path::{PathBuf, Path},
    sync::Arc, fs, str::FromStr,
};
use structopt::StructOpt;
use tokio::{spawn, sync::broadcast};
use url::{Host, Url};

const TEST_COMMITMENTS_PATH: &str = "tests/commitments.json";
const TEST_LEAFS: &'static [&'static str] = &[
    "0000000000000000000000000000000000000000000000000000000000000001",
    "0000000000000000000000000000000000000000000000000000000000000002",
];


#[tokio::test]
async fn insert_identity_and_proofs() {
    let mut options = Options::from_iter_safe(&[""]).unwrap();
    options.server.server = Url::parse("http://127.0.0.1:0/").unwrap();
    // TODO delete file before test? how to manage path?
    if Path::new(TEST_COMMITMENTS_PATH).exists() {
        fs::remove_file(TEST_COMMITMENTS_PATH).unwrap();
    }
    options.app.storage_file = PathBuf::from(TEST_COMMITMENTS_PATH);
    let local_addr = spawn_app(options.clone())
        .await
        .expect("Failed to spawn app.");
    let uri = "http://".to_owned() + &local_addr.to_string();
    let mut ref_tree = MimcTree::new(options.app.tree_depth, options.app.initial_leaf);
    let client = Client::new();

    test_inclusion_proof(&uri, &client, 0, &mut ref_tree, &options.app.initial_leaf).await;
    test_inclusion_proof(&uri, &client, 1, &mut ref_tree, &options.app.initial_leaf).await;
    test_insert_identity(
        &uri,
        &client,
        TEST_LEAFS[0],
        0,
    )
    .await;
    test_inclusion_proof(
        &uri,
        &client,
        0,
        &mut ref_tree,
        &Hash::from_str(
            TEST_LEAFS[0]
        ).unwrap(),
    )
    .await;
    test_insert_identity(
        &uri,
        &client,
        TEST_LEAFS[1],
        1,
    )
    .await;
    test_inclusion_proof(
        &uri,
        &client,
        1,
        &mut ref_tree,
        &Hash::from_str(
            TEST_LEAFS[1]
        ).unwrap(),
    )
    .await;
    test_inclusion_proof(&uri, &client, 2, &mut ref_tree, &options.app.initial_leaf).await;

    fs::remove_file(TEST_COMMITMENTS_PATH).unwrap();
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
        .unwrap();

    let mut response = client
        .request(req)
        .await
        .expect("Failed to execute request.");
    assert!(response.status().is_success());

    let bytes = hyper::body::to_bytes(response.body_mut()).await.unwrap();
    let result = String::from_utf8(bytes.into_iter().collect()).unwrap();

    ref_tree.set(leaf_index, *leaf);
    let proof = ref_tree.proof(leaf_index).expect("Ref tree malfunctioning");
    let serialized_proof =
        serde_json::to_string_pretty(&proof).expect("Proof serialization failed");

    assert_eq!(result, serialized_proof);
}

/// TODO: requires running geth node with deployed contract -- how best to mock
/// or automate
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
        .unwrap();

    let mut response = client
        .request(req)
        .await
        .expect("Failed to execute request.");
    assert!(response.status().is_success());

    let bytes = hyper::body::to_bytes(response.body_mut()).await.unwrap();
    let result = String::from_utf8(bytes.into_iter().collect()).unwrap();

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

async fn spawn_app(options: Options) -> EyreResult<SocketAddr> {
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
        let (shutdown, _) = broadcast::channel(1);
        async move {
            server::bind_from_listener(app, listener, shutdown)
                .await
                .expect("Failed to bind address");
        }
    });

    Ok(local_addr)
}
