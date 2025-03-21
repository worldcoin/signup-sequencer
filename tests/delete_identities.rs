#![allow(clippy::needless_range_loop)]

mod common;

use common::prelude::*;

use crate::common::test_delete_identity;

const IDLE_TIME: u64 = 15;

#[tokio::test]
async fn delete_identities_onchain() -> anyhow::Result<()> {
    delete_identities(false).await
}

#[tokio::test]
async fn delete_identities_offchain() -> anyhow::Result<()> {
    delete_identities(true).await
}

async fn delete_identities(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let insertion_batch_size: usize = 8;
    let deletion_batch_size: usize = 3;

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let docker = Cli::default();
    let (mock_chain, db_container, insertion_prover_map, deletion_prover_map, micro_oz) =
        spawn_deps(
            initial_root,
            &[insertion_batch_size],
            &[deletion_batch_size],
            DEFAULT_TREE_DEPTH as u8,
            &docker,
        )
        .await?;

    let mock_insertion_prover = &insertion_prover_map[&insertion_batch_size];
    let mock_deletion_prover = &deletion_prover_map[&deletion_batch_size];

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
        .add_prover(mock_insertion_prover)
        .add_prover(mock_deletion_prover)
        .offchain_mode(offchain_mode_enabled)
        .build()?;

    let (_, app_handle, local_addr, shutdown) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");

    let test_identities = generate_test_identities(insertion_batch_size * 3);
    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    // Insert enough identities to trigger an batch to be sent to the blockchain.
    for i in 0..insertion_batch_size {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, i).await;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Check that we can also get these inclusion proofs back.
    for i in 0..insertion_batch_size {
        test_inclusion_proof(
            &mock_chain,
            &uri,
            &client,
            i,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[i], 16)
                .expect("Failed to parse Hash from test leaf"),
            false,
            offchain_mode_enabled,
        )
        .await;
    }

    // Delete enough identities to trigger a batch
    for i in 0..deletion_batch_size {
        test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, i, false).await;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Ensure that identities have been deleted
    for i in 0..deletion_batch_size {
        test_inclusion_proof(
            &mock_chain,
            &uri,
            &client,
            i,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[i], 16)
                .expect("Failed to parse Hash from test leaf"),
            true,
            offchain_mode_enabled,
        )
        .await;
    }

    // Expect failure when deleting an identity that has already been deleted
    test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, 0, true).await;

    // Expect failure when deleting an identity that can not be found
    test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, 12, true).await;

    // Shutdown the app and reset the mock shutdown, allowing us to test the
    // behaviour with saved data.
    info!("Stopping the app for testing purposes");
    shutdown.shutdown();
    app_handle.await.unwrap();

    // Test loading the state from a file when the on-chain contract has the state.
    let (_, app_handle, local_addr, shutdown) =
        spawn_app(config).await.expect("Failed to spawn app.");
    let uri = "http://".to_owned() + &local_addr.to_string();

    // Ensure that identities have been deleted
    for i in 0..deletion_batch_size {
        test_inclusion_proof(
            &mock_chain,
            &uri,
            &client,
            i,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[i], 16)
                .expect("Failed to parse Hash from test leaf"),
            true,
            offchain_mode_enabled,
        )
        .await;
    }

    // Ensure that valid proofs can still be received after restart for identities
    // that have not been deleted
    for i in deletion_batch_size + 1..insertion_batch_size {
        test_inclusion_proof(
            &mock_chain,
            &uri,
            &client,
            i,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[i], 16)
                .expect("Failed to parse Hash from test leaf"),
            false,
            offchain_mode_enabled,
        )
        .await;
    }

    // Shutdown the app properly for the final time
    shutdown.shutdown();
    app_handle.await.unwrap();
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }
    for (_, prover) in deletion_prover_map.into_iter() {
        prover.stop();
    }

    Ok(())
}
