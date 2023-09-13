mod common;

use common::prelude::*;

const SUPPORTED_DEPTH: usize = 20;
const IDLE_TIME: u64 = 7;

#[tokio::test]
#[serial_test::serial]
async fn insert_identity_and_proofs() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let batch_size: usize = 3;
    #[allow(clippy::cast_possible_truncation)]
    let tree_depth: u8 = SUPPORTED_DEPTH as u8;

    let mut ref_tree = PoseidonTree::new(SUPPORTED_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) =
        spawn_deps(initial_root, &[batch_size], &vec![], tree_depth).await?;

    if let Some(insertion_prover_map) = insertion_prover_map {
        let prover_mock = &insertion_prover_map[&batch_size];

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
        ])
        .context("Failed to create options")?;

        options.server.server = Url::parse("http://127.0.0.1:0/").expect("Failed to parse URL");

        options.app.contracts.identity_manager_address = mock_chain.identity_manager.address();
        options.app.ethereum.ethereum_provider =
            Url::parse(&mock_chain.anvil.endpoint()).expect("Failed to parse Anvil url");

        let (app, local_addr) = spawn_app(options.clone())
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
            &uri,
            &client,
            0,
            &ref_tree,
            &options.app.contracts.initial_leaf_value,
            true,
        )
        .await;
        test_inclusion_proof(
            &uri,
            &client,
            1,
            &ref_tree,
            &options.app.contracts.initial_leaf_value,
            true,
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
            &uri,
            &client,
            0,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[0], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;
        test_inclusion_proof(
            &uri,
            &client,
            1,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[1], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;
        test_inclusion_proof(
            &uri,
            &client,
            2,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[2], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
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
            &uri,
            &client,
            3,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[3], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;
        test_inclusion_proof(
            &uri,
            &client,
            4,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[4], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;

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

        // Check that we can still get inclusion proofs for identities we know to have
        // been inserted previously. Here we check the first and last ones we inserted.
        test_inclusion_proof(
            &uri,
            &client,
            0,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[0], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;
        test_inclusion_proof(
            &uri,
            &client,
            4,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[4], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;

        // Shutdown the app and reset the mock shutdown, allowing us to test the
        // behaviour with the saved tree.
        info!("Stopping the app for testing purposes");
        shutdown();
        app.await.unwrap();
        reset_shutdown();

        // Test loading the state from the saved tree when the on-chain contract has the
        // state.
        let (app, local_addr) = spawn_app(options.clone())
            .await
            .expect("Failed to spawn app.");
        let uri = "http://".to_owned() + &local_addr.to_string();

        // Check that we can still get inclusion proofs for identities we know to have
        // been inserted previously. Here we check the first and last ones we inserted.
        test_inclusion_proof(
            &uri,
            &client,
            0,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[0], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;
        test_inclusion_proof(
            &uri,
            &client,
            4,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[4], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;

        // Shutdown the app properly for the final time
        shutdown();
        app.await.unwrap();
        for (_, prover) in insertion_prover_map.into_iter() {
            prover.stop();
        }
        reset_shutdown();

        Ok(())
    } else {
        Err(anyhow::anyhow!("No insertion prover map found"))
    }
}
