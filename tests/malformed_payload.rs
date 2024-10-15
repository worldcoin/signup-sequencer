mod common;

use common::prelude::*;

#[tokio::test]
async fn malformed_payload_onchain() -> anyhow::Result<()> {
    malformed_payload(false).await
}

#[tokio::test]
async fn malformed_payload_offchain() -> anyhow::Result<()> {
    malformed_payload(true).await
}

/// Tests that the app rejects payloads which are too large or are not valid
/// UTF-8 strings
async fn malformed_payload(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    init_tracing_subscriber();
    info!("Starting malformed payload test");

    let ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let batch_size: usize = 3;

    let docker = Cli::default();
    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) = spawn_deps(
        initial_root,
        &[batch_size],
        &[],
        DEFAULT_TREE_DEPTH as u8,
        &docker,
    )
    .await?;

    let prover_mock = &insertion_prover_map[&batch_size];

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
        .offchain_mode(offchain_mode_enabled)
        .build()?;

    let (_, app_handle, local_addr, shutdown) =
        spawn_app(config).await.expect("Failed to spawn app.");

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    // 20 MiB zero bytes
    let body = vec![0u8; 1024 * 1024 * 20];

    let response = client
        .post(format!("{uri}/insertIdentity"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // A KiB of 0xffs is not a valid UTF-8 string
    let body = vec![0xffu8; 1024];

    let response = client
        .post(format!("{uri}/insertIdentity"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    shutdown.shutdown();
    app_handle.await?;
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }

    Ok(())
}
