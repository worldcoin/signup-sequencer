mod common;

use common::prelude::*;

/// Tests that the app can keep running even if the prover returns 500s
/// and that it will eventually succeed if the prover becomes available again.
#[tokio::test]
async fn unavailable_prover() -> anyhow::Result<()> {
    init_tracing_subscriber();
    info!("Starting unavailable prover test");

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH as usize + 1, ruint::Uint::ZERO);
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

    let (app, local_addr) = spawn_app(config).await.expect("Failed to spawn app.");

    let test_identities = generate_test_identities(batch_size * 2);
    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    // Insert enough identities to trigger an batch to be sent to the blockchain
    // based on the current batch size of 3.
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 0).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 1).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 2).await;

    // Wait for a while - this should let the processing thread panic or fail at
    // least once
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Make prover available again
    prover_mock.set_availability(true).await;
    // and wait until the processing thread spins up again
    tokio::time::sleep(Duration::from_secs(5)).await;

    info!("Prover has been reenabled");

    // Test that the identities have been inserted and processed
    test_inclusion_proof(&uri, &client, 0, &ref_tree, &identities_ref[0], false).await;
    test_inclusion_proof(&uri, &client, 1, &ref_tree, &identities_ref[1], false).await;
    test_inclusion_proof(&uri, &client, 2, &ref_tree, &identities_ref[2], false).await;

    shutdown();
    app.await?;
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
