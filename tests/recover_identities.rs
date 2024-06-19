#![allow(clippy::needless_range_loop)]

mod common;

use common::prelude::*;

use crate::common::{test_in_tree, test_not_in_tree, test_recover_identity};

const IDLE_TIME: u64 = 7;

#[tokio::test]
async fn recover_identities_onchain() -> anyhow::Result<()> {
    recover_identities(false).await
}

async fn recover_identities(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let insertion_batch_size: usize = 8;
    let deletion_batch_size: usize = 3;

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH + 1, ruint::Uint::ZERO);
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

    // Set the root history expirty to 15 seconds
    let updated_root_history_expiry = U256::from(30);
    mock_chain
        .identity_manager
        .method::<_, ()>("setRootHistoryExpiry", updated_root_history_expiry)?
        .send()
        .await?
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
        .min_batch_deletion_size(deletion_batch_size)
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .add_prover(mock_insertion_prover)
        .add_prover(mock_deletion_prover)
        .offchain_mode(offchain_mode_enabled)
        .build()?;

    let (_, app_handle, local_addr, shutdown) =
        spawn_app(config).await.expect("Failed to spawn app.");

    let test_identities = generate_test_identities(insertion_batch_size * 3);
    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    let mut next_leaf_index = 0;
    // Insert enough identities to trigger an batch to be sent to the blockchain.
    for _ in 0..insertion_batch_size {
        test_insert_identity(
            &uri,
            &client,
            &mut ref_tree,
            &identities_ref,
            next_leaf_index,
        )
        .await;

        next_leaf_index += 1;
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
            &identities_ref[i],
            false,
        )
        .await;
    }

    // Test that we cannot recover with an identity that has previously been
    // inserted
    test_recover_identity(
        &uri,
        &client,
        &mut ref_tree,
        &identities_ref,
        // Last inserted identity
        insertion_batch_size - 1,
        // Second to last inserted identity as recovery
        identities_ref[insertion_batch_size - 2],
        next_leaf_index,
        true,
    )
    .await;

    // Insert enough recoveries to trigger a batch
    for i in 0..deletion_batch_size {
        // Delete the identity at i and replace it with an identity at the back of the
        //  test identities array
        // TODO: we should update to a much cleaner approach

        test_recover_identity(
            &uri,
            &client,
            &mut ref_tree,
            &identities_ref,
            i,
            identities_ref[next_leaf_index],
            next_leaf_index,
            false,
        )
        .await;

        next_leaf_index += 1;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME * 3)).await;

    // Ensure that identities have been deleted
    for i in 0..deletion_batch_size {
        test_inclusion_proof(
            &mock_chain,
            &uri,
            &client,
            i,
            &ref_tree,
            &identities_ref[i],
            true,
        )
        .await;

        // Check that the replacement identity has not been inserted yet
        let recovery_leaf_index = insertion_batch_size + i;
        test_not_in_tree(&uri, &client, &identities_ref[recovery_leaf_index]).await;
    }

    // // Update ref tree with changes that will be done in background
    // for i in 0..deletion_batch_size {
    //     let recovery_leaf_index = insertion_batch_size + i;
    //     ref_tree.set(recovery_leaf_index, identities_ref[recovery_leaf_index]);
    // }

    // Sleep for root expiry
    tokio::time::sleep(Duration::from_secs(updated_root_history_expiry.as_u64())).await;

    // Insert enough identities to trigger a batch to be sent to the blockchain.
    for _ in 0..insertion_batch_size {
        test_insert_identity(
            &uri,
            &client,
            &mut ref_tree,
            &identities_ref,
            next_leaf_index,
        )
        .await;
        next_leaf_index += 1;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME * 8)).await;

    // Check that the replacement identities have been inserted
    for i in 0..deletion_batch_size {
        let recovery_leaf_index = insertion_batch_size + i;

        // Check that the replacement identity has a mined status after an insertion
        // batch has completed
        test_in_tree(&uri, &client, &identities_ref[recovery_leaf_index]).await;
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
