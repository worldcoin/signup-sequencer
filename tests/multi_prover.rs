mod common;

use common::prelude::*;

#[tokio::test]
async fn multi_prover_onchain() -> anyhow::Result<()> {
    multi_prover(false).await
}

#[tokio::test]
async fn multi_prover_offchain() -> anyhow::Result<()> {
    multi_prover(true).await
}

/// Tests that the app can keep running even if the prover returns 500s
/// and that it will eventually succeed if the prover becomes available again.
async fn multi_prover(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    init_tracing_subscriber();
    info!("Starting multi prover test");

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let batch_timeout_seconds = Duration::from_secs(20);
    let insert_task_delay_seconds = Duration::from_secs(5);
    let sync_task_delay_seconds = Duration::from_secs(5);
    let jitter_delay_seconds = Duration::from_secs(1);
    let ensure_batch_seconds = batch_timeout_seconds
        + insert_task_delay_seconds
        + sync_task_delay_seconds
        + jitter_delay_seconds;

    let batch_size_3: usize = 3;
    let batch_size_10: usize = 10;

    let docker = Cli::default();
    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) = spawn_deps(
        initial_root,
        &[batch_size_3, batch_size_10],
        &[],
        DEFAULT_TREE_DEPTH as u8,
        &docker,
    )
    .await?;

    let prover_mock_batch_size_3 = &insertion_prover_map[&batch_size_3];
    let prover_mock_batch_size_10 = &insertion_prover_map[&batch_size_10];

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
        .batch_insertion_timeout(batch_timeout_seconds)
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .add_prover(prover_mock_batch_size_3)
        .add_prover(prover_mock_batch_size_10)
        .offchain_mode(offchain_mode_enabled)
        .build()?;

    tracing::info!("Spawning app");
    let (_, app_handle, local_addr, shutdown) =
        spawn_app(config).await.expect("Failed to spawn app.");

    let test_identities = generate_test_identities(1 + batch_size_3 + batch_size_10);

    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    // Insert only 1 identity, so that the sequencer will create non-full batch of 1 because
    // in tests we have clean database and we have not inserted anything before
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 0).await;

    // Wait until a batch will be created for sure
    tokio::time::sleep(ensure_batch_seconds).await;

    // Identities should have been inserted and processed
    for (i, identity) in identities_ref.iter().enumerate().take(1) {
        test_inclusion_proof(
            &mock_chain,
            &uri,
            &client,
            i,
            &ref_tree,
            identity,
            false,
            offchain_mode_enabled,
        )
        .await;
    }

    // Test if we will use batch size of 3 (minimal batch size that can handle
    // batch in one request) for 3 identities being inserted.
    // We're disabling the larger prover, so that only inserting to the smaller
    // batch size 3 prover can work
    prover_mock_batch_size_10.set_availability(false).await;
    prover_mock_batch_size_3.set_availability(true).await; // on by default, but here for visibility

    let offset = 1;
    for i in 0..batch_size_3 {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, offset + i).await;
    }

    // Wait until a batch can be produced
    tokio::time::sleep(ensure_batch_seconds).await;

    // Identities should have been inserted and processed
    for i in 0..batch_size_3 {
        test_inclusion_proof(
            &mock_chain,
            &uri,
            &client,
            offset + i,
            &ref_tree,
            &identities_ref[i + offset],
            false,
            offchain_mode_enabled,
        )
        .await;
    }

    // Test if we will use batch size of 10 (minimal batch size that can handle
    // batch in one request) for 10 identities being inserted.
    // Now re re-enable the larger prover and disable the smaller one
    prover_mock_batch_size_10.set_availability(true).await;
    prover_mock_batch_size_3.set_availability(false).await;

    let offset = offset + batch_size_3;
    for i in 0..batch_size_10 {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, offset + i).await;
    }

    // Wait until a batch can be produced
    tokio::time::sleep(ensure_batch_seconds).await;

    // Identities should have been inserted and processed
    for i in 0..batch_size_10 {
        test_inclusion_proof(
            &mock_chain,
            &uri,
            &client,
            offset + i,
            &ref_tree,
            &identities_ref[i + offset],
            false,
            offchain_mode_enabled,
        )
        .await;
    }

    shutdown.shutdown();
    app_handle.await?;
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }

    Ok(())
}
