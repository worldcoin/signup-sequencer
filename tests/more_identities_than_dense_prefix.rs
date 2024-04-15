mod common;

use common::prelude::*;
use testcontainers::clients::Cli;

const IDLE_TIME: u64 = 12;

#[tokio::test]
async fn more_identities_than_dense_prefix() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let tree_depth = 20;
    let dense_prefix = 3;

    let batch_size: usize = 4;

    // 2^3 = 8, so 2 batches
    let num_identities_in_dense_prefix = 2usize.pow(dense_prefix as u32);
    let num_identities_above_dense_prefix = batch_size * 2;

    // A total of 4 batches (4 * 4 = 16 identities)
    let num_identities_total = num_identities_in_dense_prefix + num_identities_above_dense_prefix;

    let num_batches_total = num_identities_total / batch_size;

    let mut ref_tree = PoseidonTree::new(tree_depth + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let docker = Cli::default();
    let (mock_chain, db_container, prover_map, _deletion_prover_map, micro_oz) =
        spawn_deps(initial_root, &[batch_size], &[], tree_depth as u8, &docker).await?;

    let prover_mock = &prover_map[&batch_size];

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
        .tree_depth(tree_depth)
        .dense_tree_prefix_depth(dense_prefix)
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .add_prover(prover_mock)
        .build()?;

    let (_, app_handle, local_addr) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");

    let test_identities = generate_test_identities(num_identities_total);
    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    info!("############# Insert all the identities #############");

    // Insert identities to fill out the dense prefix
    for i in 0..num_identities_total {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, i).await;
    }

    // Sleep long enough to process all the batches
    tokio::time::sleep(Duration::from_secs(IDLE_TIME * num_batches_total as u64)).await;

    // Check that we can get inclusion proof for the first identity
    test_inclusion_proof(&uri, &client, 0, &ref_tree, &identities_ref[0], false).await;

    // Check that we can get inclusion proof for the last identity
    test_inclusion_proof(
        &uri,
        &client,
        num_identities_total - 1,
        &ref_tree,
        &identities_ref[num_identities_total - 1],
        false,
    )
    .await;

    info!("############# Restart the sequencer - triggers a tree restore #############");

    // Shutdown the app and reset the mock shutdown, allowing us to test the
    // behaviour with saved data.
    info!("Stopping the app for testing purposes");
    shutdown();
    app_handle.await.unwrap();
    reset_shutdown();

    // Test loading the state from a file when the on-chain contract has the state.
    let (_, app_handle, local_addr) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");
    let uri = "http://".to_owned() + &local_addr.to_string();

    info!("############# Validate restored tree #############");

    // After app restart, the tree should have been restored
    // and we should still have all the inserted identities

    // Sleep long enough for the app tree to be restored
    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Check that we can get inclusion proof for the first identity
    test_inclusion_proof(&uri, &client, 0, &ref_tree, &identities_ref[0], false).await;

    // Check that we can get inclusion proof for the last identity
    test_inclusion_proof(
        &uri,
        &client,
        num_identities_total - 1,
        &ref_tree,
        &identities_ref[num_identities_total - 1],
        false,
    )
    .await;

    // Shutdown the app properly for the final time
    shutdown();
    app_handle.await.unwrap();
    for (_, prover) in prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
