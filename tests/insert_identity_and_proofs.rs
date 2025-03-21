mod common;

use common::prelude::*;

const IDLE_TIME: u64 = 15;

#[tokio::test]
async fn insert_identity_and_proofs_onchain() -> anyhow::Result<()> {
    insert_identity_and_proofs(false).await
}

#[tokio::test]
async fn insert_identity_and_proofs_offchain() -> anyhow::Result<()> {
    insert_identity_and_proofs(true).await
}

async fn insert_identity_and_proofs(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let batch_size: usize = 3;

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

    let (_, app_handle, local_addr, shutdown) = spawn_app(config.clone())
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
        &mock_chain,
        &uri,
        &client,
        0,
        &ref_tree,
        &config.tree.initial_leaf_value,
        true,
        offchain_mode_enabled,
    )
    .await;
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        1,
        &ref_tree,
        &config.tree.initial_leaf_value,
        true,
        offchain_mode_enabled,
    )
    .await;

    // Insert enough identities to trigger an batch to be sent to the blockchain
    // based on the current batch size of 3.
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 0).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 1).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 2).await;

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;
    // Check that we can get their inclusion proofs back.
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        0,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[0], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
        offchain_mode_enabled,
    )
    .await;
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        1,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[1], 16)
            .expect("Failed to parse Hash from test leaf 1"),
        false,
        offchain_mode_enabled,
    )
    .await;
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        2,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[2], 16)
            .expect("Failed to parse Hash from test leaf 2"),
        false,
        offchain_mode_enabled,
    )
    .await;

    // Insert too few identities to trigger a batch, and then force the timeout to
    // complete and submit a partial batch to the chain.
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 3).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 4).await;
    tokio::time::pause();
    tokio::time::resume();

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;
    // Check that we can also get these inclusion proofs back.
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        3,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[3], 16)
            .expect("Failed to parse Hash from test leaf 3"),
        false,
        offchain_mode_enabled,
    )
    .await;
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        4,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[4], 16)
            .expect("Failed to parse Hash from test leaf 4"),
        false,
        offchain_mode_enabled,
    )
    .await;

    // Shutdown the app and reset the mock shutdown, allowing us to test the
    // behaviour with saved data.
    info!("Stopping the app for testing purposes");
    shutdown.shutdown();
    app_handle.await.unwrap();

    // Test loading the state from a file when the on-chain contract has the state.
    let (_, app_handle, local_addr, shutdown) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");
    let uri = "http://".to_owned() + &local_addr.to_string();

    // Check that we can still get inclusion proofs for identities we know to have
    // been inserted previously. Here we check the first and last ones we inserted.
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        0,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[0], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
        offchain_mode_enabled,
    )
    .await;
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        4,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[4], 16)
            .expect("Failed to parse Hash from test leaf 4"),
        false,
        offchain_mode_enabled,
    )
    .await;

    // Shutdown the app and reset the mock shutdown, allowing us to test the
    // behaviour with the saved tree.
    info!("Stopping the app for testing purposes");
    shutdown.shutdown();
    app_handle.await.unwrap();

    // Test loading the state from the saved tree when the on-chain contract has the
    // state.
    let (_, app_handle, local_addr, shutdown) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");
    let uri = "http://".to_owned() + &local_addr.to_string();

    // Check that we can still get inclusion proofs for identities we know to have
    // been inserted previously. Here we check the first and last ones we inserted.
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        0,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[0], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
        offchain_mode_enabled,
    )
    .await;
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        4,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[4], 16)
            .expect("Failed to parse Hash from test leaf 4"),
        false,
        offchain_mode_enabled,
    )
    .await;

    // Shutdown the app properly for the final time
    shutdown.shutdown();
    app_handle.await.unwrap();
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }

    Ok(())
}
