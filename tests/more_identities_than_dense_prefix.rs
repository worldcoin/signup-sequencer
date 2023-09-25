mod common;

use common::prelude::*;

const SUPPORTED_DEPTH: usize = 20;
const IDLE_TIME: u64 = 12;

#[tokio::test]
async fn more_identities_than_dense_prefix() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let batch_size: usize = 4;
    let dense_prefix_depth: usize = 3;

    // 2^3 = 8, so 2 batches
    let num_identities_in_dense_prefix = 2usize.pow(dense_prefix_depth as u32);
    let num_identities_above_dense_prefix = batch_size * 2;

    // A total of 4 batches (4 * 4 = 16 identities)
    let num_identities_total = num_identities_in_dense_prefix + num_identities_above_dense_prefix;

    let num_batches_total = num_identities_total / batch_size;

    #[allow(clippy::cast_possible_truncation)]
    let tree_depth: u8 = SUPPORTED_DEPTH as u8;

    let mut ref_tree = PoseidonTree::new(SUPPORTED_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let (mock_chain, db_container, prover_map, _deletion_prover_map, micro_oz) =
        spawn_deps(initial_root, &[batch_size], &[], tree_depth).await?;

    let prover_mock = &prover_map[&batch_size];

    let port = db_container.port();
    let db_url = format!("postgres://postgres:postgres@localhost:{port}/database");

    // temp dir will be deleted on drop call
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
        &format!("{dense_prefix_depth}"),
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
        "--dense-tree-mmap-file",
        temp_dir.path().join("testfile").to_str().unwrap(),
    ])
    .context("Failed to create options")?;

    options.server.server = Url::parse("http://127.0.0.1:0/").expect("Failed to parse URL");

    options.app.contracts.identity_manager_address = mock_chain.identity_manager.address();
    options.app.ethereum.ethereum_provider = Url::parse(&mock_chain.anvil.endpoint())?;

    let (app, local_addr) = spawn_app(options.clone())
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
    app.await.unwrap();
    reset_shutdown();

    // Test loading the state from a file when the on-chain contract has the state.
    let (app, local_addr) = spawn_app(options.clone())
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
    app.await.unwrap();
    for (_, prover) in prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
