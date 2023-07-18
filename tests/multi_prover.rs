mod common;

use common::prelude::*;

/// Tests that the app can keep running even if the prover returns 500s
/// and that it will eventually succeed if the prover becomes available again.
#[tokio::test]
async fn multi_prover() -> anyhow::Result<()> {
    init_tracing_subscriber();
    info!("Starting unavailable prover test");

    let tree_depth: u8 = 20;

    let mut ref_tree = PoseidonTree::new(tree_depth as usize + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let batch_timeout_seconds: u64 = 11;

    let batch_size_3: usize = 3;
    let batch_size_10: usize = 10;

    let (mock_chain, db_container, prover_map, micro_oz) =
        spawn_deps(initial_root, &[batch_size_3, batch_size_10], tree_depth).await?;

    let prover_mock_batch_size_3 = &prover_map[&batch_size_3];
    let prover_mock_batch_size_10 = &prover_map[&batch_size_10];

    let prover_arg_string = format!(
        "[{},{}]",
        prover_mock_batch_size_3.arg_string_single(),
        prover_mock_batch_size_10.arg_string_single()
    );

    info!("Running with {prover_arg_string}");

    let port = db_container.port();
    let db_url = format!("postgres://postgres:postgres@localhost:{port}/database");
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
        &prover_arg_string,
        "--batch-timeout-seconds",
        &format!("{batch_timeout_seconds}"),
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
    ])
    .context("Failed to create options")?;

    options.server.server = Url::parse("http://127.0.0.1:0/")?;

    options.app.contracts.identity_manager_address = mock_chain.identity_manager.address();
    options.app.ethereum.ethereum_provider = Url::parse(&mock_chain.anvil.endpoint())?;

    let (app, local_addr) = spawn_app(options.clone())
        .await
        .expect("Failed to spawn app.");

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
    app.await?;
    for (_, prover) in prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
