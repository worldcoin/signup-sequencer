use hyper::{Body, Client, Request};
use rust_app_template::{app::App, mimc_tree::MimcTree, server, Options};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use structopt::StructOpt;
use tokio::{spawn, sync::broadcast};

#[tokio::test]
async fn insert_identity_works() {
    let mut options = Options::from_iter_safe(&[""]).unwrap();
    // TODO delete file before test? how to manage path?
    options.app.storage_file = PathBuf::from("tests/commitments.json");
    spawn_app(options.clone()).await;
    let port = options.server.server.port().unwrap().to_string();
    let client = Client::new();
    let body = construct_inclusion_proof_body(0);
    let req = Request::builder()
        .method("POST")
        .uri("http://localhost:".to_owned() + &port + "/inclusionProof")
        .header("Content-Type", "application/json")
        .body(body)
        .unwrap();

    let ref_tree = MimcTree::new(options.app.tree_depth, options.app.initial_leaf);

    let mut response = client
        .request(req)
        .await
        .expect("Failed to execute request.");
    assert!(response.status().is_success());
    let bytes = hyper::body::to_bytes(response.body_mut()).await.unwrap();
    let result = String::from_utf8(bytes.into_iter().collect()).unwrap();
    let proof = ref_tree.proof(0).expect("Ref tree malfunctioning");
    let serialized_proof =
        serde_json::to_string_pretty(&proof).expect("Proof serialization failed");

    assert_eq!(result, serialized_proof);
}

fn construct_inclusion_proof_body(identity_index: usize) -> Body {
    Body::from(
        json!({
            "identityIndex": identity_index,
        })
        .to_string(),
    )
}

async fn spawn_app(options: Options) -> url::Url {
    let app = Arc::new(App::new(options.app).await.expect("Failed to create App"));
    let server = options.server.server.clone();
    spawn({
        let (shutdown, _) = broadcast::channel(1);
        async move {
            server::main(app, options.server, shutdown)
                .await
                .expect("Start");
        }
    });
    server
}
