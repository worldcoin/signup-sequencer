use std::sync::Arc;
// use rust_app_template::app;

use hyper::{Request, Body, Client};
use rust_app_template::{Options, app::App, server};
use serde_json::json;
use structopt::StructOpt;
use tokio::{spawn, sync::broadcast};



#[tokio::test]
async fn insert_identity_works() {
    spawn_app().await;
    let client = Client::new();
    let body = construct_inclusion_proof_body(0);
    let req = Request::builder()
        .method("POST")
        .uri("http://localhost:8080/inclusionProof")
        .header("Content-Type", "application/json")
        .body(body)
        .unwrap();

    let response = client.request(req).await.expect("Failed to execute request.");
    assert!(response.status().is_success());
}

fn construct_inclusion_proof_body(identity_index: usize) -> Body{
    Body::from(
        json!({
            "identityIndex": identity_index,
        })
        .to_string(),
    )
}

async fn spawn_app() {
    let options = Options::from_iter_safe(&[""]).unwrap();
    let app = Arc::new(App::new(options.app).await.expect("Failed to create App"));
    spawn({
        let (shutdown, _) = broadcast::channel(1);
        async move {
            server::main(app, options.server, shutdown).await.expect("Start");
        }
    });
}
