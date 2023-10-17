mod common;
use common::prelude::*;

#[tokio::test]
async fn test_unreduced_identity() -> anyhow::Result<()> {
    info!("Starting unavailable prover test");

    let tree_depth: u8 = 20;

    let ref_tree = PoseidonTree::new(tree_depth as usize + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();
    let batch_size: usize = 3;

    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) =
        spawn_deps(initial_root, &[batch_size], &[], tree_depth).await?;
    let prover_mock = &insertion_prover_map[&batch_size];
    prover_mock.set_availability(false).await;

    let db_socket_addr = db_container.address();
    let db_url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

    let temp_dir = tempfile::tempdir()?;
    info!(
        "temp dir created at: {:?}",
        temp_dir.path().join("testfile")
    );

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
        "--oz-api-key",
        "",
        "--oz-api-secret",
        "",
        "--oz-api-url",
        &micro_oz.endpoint(),
        "--oz-address",
        &format!("{:?}", micro_oz.address()),
        "--time-between-scans-seconds",
        "1",
        "--dense-tree-mmap-file",
        temp_dir.path().join("testfile").to_str().unwrap(),
    ])
    .context("Failed to create options")?;

    options.server.server = Url::parse("http://127.0.0.1:0/")?;

    options.app.contracts.identity_manager_address = mock_chain.identity_manager.address();
    options.app.ethereum.ethereum_provider = Url::parse(&mock_chain.anvil.endpoint())?;

    let (app, local_addr) = spawn_app(options.clone())
        .await
        .expect("Failed to spawn app.");

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    // Test unreduced identity for insertion
    let body = common::construct_insert_identity_body(&ruint::Uint::<256, 4>::MAX);
    let req = Request::builder()
        .method("POST")
        .uri(uri.to_owned() + "/insertIdentity")
        .header("Content-Type", "application/json")
        .body(body)
        .expect("Failed to create insert identity hyper::Body");

    let response = client
        .request(req)
        .await
        .expect("Failed to execute request.");

    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("Failed to read body bytes");
    let body_str = String::from_utf8_lossy(&bytes);

    assert_eq!(
        "provided identity commitment is not in reduced form",
        body_str
    );

    // Test unreduced identity for recovery
    let body = common::construct_recover_identity_body(&Hash::ZERO, &ruint::Uint::<256, 4>::MAX);
    let req = Request::builder()
        .method("POST")
        .uri(uri.to_owned() + "/recoverIdentity")
        .header("Content-Type", "application/json")
        .body(body)
        .expect("Failed to create insert identity hyper::Body");

    let response = client
        .request(req)
        .await
        .expect("Failed to execute request.");

    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("Failed to read body bytes");
    let body_str = String::from_utf8_lossy(&bytes);

    assert_eq!(
        "provided identity commitment is not in reduced form",
        body_str
    );

    shutdown();
    app.await?;
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
