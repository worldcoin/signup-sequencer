mod common;

use common::prelude::*;

const IDLE_TIME: u64 = 15;

#[tokio::test]
async fn tree_restore_one_commitment_onchain() -> anyhow::Result<()> {
    tree_restore_one_commitment(false).await
}

#[tokio::test]
async fn tree_restore_one_commitment_offchain() -> anyhow::Result<()> {
    tree_restore_one_commitment(true).await
}

async fn tree_restore_one_commitment(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let batch_size: usize = 1;

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

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

    // temp dir will be deleted on drop call
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

    let (app, app_handle, local_addr, shutdown) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");

    let test_identities = generate_test_identities(1);
    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 0).await;

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Await for tree to be mined in app
    let tree_state = await_tree_state_with_mined_leafs_size(&app, 1).await?;

    assert_eq!(tree_state.latest_tree().next_leaf(), 1);

    // Shutdown the app and reset the mock shutdown, allowing us to test the
    // behaviour with saved data.
    info!("Stopping the app for testing purposes");
    shutdown.shutdown();
    app_handle.await.unwrap();

    let (app, app_handle, _, shutdown) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");

    let restored_tree_state = app.tree_state()?.clone();

    test_same_tree_states(tree_state, &restored_tree_state).await?;

    // Shutdown the app properly for the final time
    shutdown.shutdown();
    app_handle.await.unwrap();
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }

    Ok(())
}
