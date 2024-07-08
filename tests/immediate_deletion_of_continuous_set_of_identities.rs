#![allow(clippy::needless_range_loop)]

mod common;

use std::time::Duration;

use common::prelude::*;
use common::test_inclusion_proof_mined;

use crate::common::test_delete_identity;

const IDLE_TIME: u64 = 7;

/// This test ensures that we're safe against a scenario where we delete a set
/// of identities which were just inserted.
///
/// Example:
/// Let's say we insert identities at indexes 0, 1, 2, 3, 4 in batches of 0, 1,
/// 2 and 3, 4. We then delete identites at indexes 4 and 3 - this will result
/// in the same batch post root as from the insertion batch 0, 1, 2. This breaks
/// a central assumption that roots are unique.
#[tokio::test]
async fn immediate_deletion_of_continuous_set_of_identities_onchain() -> anyhow::Result<()> {
    immediate_deletion_of_continuous_set_of_identities(false).await
}

#[tokio::test]
async fn immediate_deletion_of_continuous_set_of_identities_offchain() -> anyhow::Result<()> {
    immediate_deletion_of_continuous_set_of_identities(true).await
}

async fn immediate_deletion_of_continuous_set_of_identities(
    offchain_mode_enabled: bool,
) -> anyhow::Result<()> {
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
        .batch_deletion_timeout(Duration::from_secs(1)) // We'll make deletion timeout really short
        .add_prover(mock_insertion_prover)
        .add_prover(mock_deletion_prover)
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

    // Delete 2 identities from the top - this could create a duplicate root
    test_delete_identity(
        &uri,
        &client,
        &mut ref_tree,
        &identities_ref,
        insertion_batch_size - 1,
        false,
    )
    .await;
    test_delete_identity(
        &uri,
        &client,
        &mut ref_tree,
        &identities_ref,
        insertion_batch_size - 2,
        false,
    )
    .await;

    tokio::time::sleep(Duration::from_secs(IDLE_TIME * 2)).await;

    // Ensure the identity has not yet been deleted
    test_inclusion_proof_mined(
        &mock_chain,
        &uri,
        &client,
        &Hash::from_str_radix(&test_identities[insertion_batch_size - 1], 16)
            .expect("Failed to parse Hash from test leaf"),
        false,
        offchain_mode_enabled,
    )
    .await;

    // Delete another identity to trigger a deletion batch - crucially there must be
    // gaps in deletions, or a new insertion must happen in between
    test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, 0, false).await;

    tokio::time::sleep(Duration::from_secs(IDLE_TIME * 2)).await;

    test_inclusion_proof_mined(
        &mock_chain,
        &uri,
        &client,
        &Hash::from_str_radix(&test_identities[insertion_batch_size - 1], 16)
            .expect("Failed to parse Hash from test leaf"),
        true,
        offchain_mode_enabled,
    )
    .await;

    // Shutdown the app and reset the mock shutdown, allowing us to test the
    // behaviour with saved data.
    info!("Stopping the app for testing purposes");
    shutdown.shutdown();
    app_handle.await.unwrap();

    Ok(())
}
