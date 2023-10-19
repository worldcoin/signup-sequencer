mod common;

use common::prelude::*;

/// Tests that the app can keep running even if the prover returns 500s
/// and that it will eventually succeed if the prover becomes available again.
#[tokio::test]
async fn unavailable_prover() -> anyhow::Result<()> {
    init_tracing_subscriber();
    info!("Starting unavailable prover test");

    let tree_depth: u8 = 20;

    let mut ref_tree = PoseidonTree::new(tree_depth as usize + 1, ruint::Uint::ZERO);
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
