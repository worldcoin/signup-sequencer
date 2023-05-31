mod common;

use common::prelude::*;
use hyper::StatusCode;

/// Tests that the app rejects payloads which are too large or are not valid
/// UTF-8 strings
#[tokio::test]
async fn malformed_payload() -> anyhow::Result<()> {
    init_tracing_subscriber();
    info!("Starting malformed payload test");

    let tree_depth: u8 = 20;

    let ref_tree = PoseidonTree::new(tree_depth as usize + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let batch_size: usize = 3;

    let (mock_chain, db_container, prover_map) =
        spawn_deps(initial_root, &[batch_size], tree_depth).await?;

    let prover_mock = &prover_map[&batch_size];

    let port = db_container.port();
    let db_url = format!("postgres://postgres:postgres@localhost:{port}/database");
    let mut options = Options::try_parse_from([
        "signup-sequencer",
        "--identity-manager-address",
        "0x0000000000000000000000000000000000000000", // placeholder, updated below
        "--database",
        &db_url,
        "--database-max-connections",
        "1",
        "--tree-depth",
        &format!("{tree_depth}"),
        "--prover-urls",
        &prover_mock.arg_string(),
        "--batch-timeout-seconds",
        "10",
        "--dense-tree-prefix-depth",
        "10",
        "--tree-gc-threshold",
        "1",
    ])
    .context("Failed to create options")?;

    options.server.server = Url::parse("http://127.0.0.1:0/")?;

    options.app.contracts.identity_manager_address = mock_chain.identity_manager.address();
    options.app.ethereum.read_options.confirmation_blocks_delay = 2;
    options.app.ethereum.read_options.ethereum_provider = Url::parse(&mock_chain.anvil.endpoint())?;

    options.app.ethereum.write_options.signing_key = mock_chain.private_key;

    let (app, local_addr) = spawn_app(options.clone())
        .await
        .expect("Failed to spawn app.");

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    // 20 MiB zero bytes
    let body = vec![0u8; 1024 * 1024 * 20];

    let too_large_payload = Request::builder()
        .method("POST")
        .uri(format!("{uri}/insertIdentity"))
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = client.request(too_large_payload).await?;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // A KiB of zeros is not a valid UTF-8 string
    let body = vec![0u8; 1024];

    let invalid_payload = Request::builder()
        .method("POST")
        .uri(format!("{uri}/insertIdentity"))
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = client.request(invalid_payload).await?;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    shutdown();
    app.await?;
    for (_, prover) in prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
