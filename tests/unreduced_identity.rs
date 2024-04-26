mod common;
use common::prelude::*;

#[tokio::test]
async fn test_unreduced_identity() -> anyhow::Result<()> {
    info!("Starting unavailable prover test");

    let ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();
    let batch_size: usize = 3;

    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) =
        spawn_deps(initial_root, &[batch_size], &[], DEFAULT_TREE_DEPTH as u8).await?;
    let prover_mock = &insertion_prover_map[&batch_size];
    prover_mock.set_availability(false).await;

    let db_socket_addr = db_container.address();
    let db_url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

    let temp_dir = tempfile::tempdir()?;
    info!(
        "temp dir created at: {:?}",
        temp_dir.path().join("testfile")
    );

    let config = TestConfigBuilder::new()
        .db_url(&db_url)
        .oz_api_url(&micro_oz.endpoint())
        .oz_address(micro_oz.address())
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .add_prover(prover_mock)
        .build()?;

    let (_, app_handle, local_addr) = spawn_app(config).await.expect("Failed to spawn app.");

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
    app_handle.await?;
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
