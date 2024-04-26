mod common;

use common::prelude::*;

/// Tests that the app can keep running even if the prover returns 500s
/// and that it will eventually succeed if the prover becomes available again.
#[tokio::test]
async fn multi_prover() -> anyhow::Result<()> {
    init_tracing_subscriber();
    info!("Starting multi prover test");

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let batch_timeout_seconds: u64 = 11;

    let batch_size_3: usize = 3;
    let batch_size_10: usize = 10;

    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) = spawn_deps(
        initial_root,
        &[batch_size_3, batch_size_10],
        &[],
        DEFAULT_TREE_DEPTH as u8,
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
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .add_prover(prover_mock_batch_size_3)
        .add_prover(prover_mock_batch_size_10)
        .build()?;

    tracing::info!("Spawning app");
    let (_, app_handle, local_addr) = spawn_app(config).await.expect("Failed to spawn app.");

    let test_identities = generate_test_identities(batch_size_3 + batch_size_10);

    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    // We're disabling the larger prover, so that only inserting to the smaller
    // batch size 3 prover can work
    prover_mock_batch_size_10.set_availability(false).await;
    prover_mock_batch_size_3.set_availability(true).await; // on by default, but here for visibility

    // Insert only 3 identities, so that the sequencer is forced to submit a batch
    // size of 3
    for i in 0..batch_size_3 {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, i).await;
    }

    // Wait until a batch can be produced
    tokio::time::sleep(Duration::from_secs(batch_timeout_seconds)).await;

    // Identities should have been inserted and processed
    for (i, identity) in identities_ref.iter().enumerate().take(batch_size_3) {
        test_inclusion_proof(&uri, &client, i, &ref_tree, identity, false).await;
    }

    // Now re re-enable the larger prover and disable the smaller one
    prover_mock_batch_size_10.set_availability(true).await;
    prover_mock_batch_size_3.set_availability(false).await;

    // Insert 10 identities

    let offset = batch_size_3;
    for i in 0..batch_size_10 {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, offset + i).await;
    }

    tokio::time::sleep(Duration::from_secs(batch_timeout_seconds)).await;

    // Identities should have been inserted and processed
    for i in 0..batch_size_10 {
        test_inclusion_proof(
            &uri,
            &client,
            offset + i,
            &ref_tree,
            &identities_ref[i + offset],
            false,
        )
        .await;
    }

    shutdown();
    app_handle.await?;
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
