#![allow(clippy::needless_range_loop)]

mod common;

use common::prelude::*;

use crate::common::test_delete_identity;

const IDLE_TIME: u64 = 10;

#[tokio::test]
async fn insert_not_full_identities_batch_when_deletion_batch_is_ahead_onchain(
) -> anyhow::Result<()> {
    insert_not_full_identities_batch_when_deletion_batch_is_ahead(false).await
}

#[tokio::test]
async fn insert_not_full_identities_batch_when_deletion_batch_is_ahead_offchain(
) -> anyhow::Result<()> {
    insert_not_full_identities_batch_when_deletion_batch_is_ahead(true).await
}

async fn insert_not_full_identities_batch_when_deletion_batch_is_ahead(
    offchain_mode_enabled: bool,
) -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let insertion_batch_size: usize = 16;
    let deletion_batch_size: usize = 8;

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
        .batch_deletion_timeout(Duration::from_secs(3600))
        .batch_insertion_timeout(Duration::from_secs(3600))
        .min_batch_deletion_size(4)
        .build()?;

    let (_, app_handle, local_addr, shutdown) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");

    let test_identities = generate_test_identities(16 + 18);
    let test_identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    // Insert one identity to trigger first batch
    test_insert_identity(&uri, &client, &mut ref_tree, &test_identities_ref, 0).await;

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        0,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[0], 16)
            .expect("Failed to parse Hash from test leaf"),
        false,
        offchain_mode_enabled,
    )
    .await;

    tokio::time::sleep(Duration::from_secs(2 * IDLE_TIME)).await;

    // Insert enough identities to trigger an batch to be sent to the blockchain to have identities
    // to delete
    for i in 1..(insertion_batch_size + 1) {
        test_insert_identity(&uri, &client, &mut ref_tree, &test_identities_ref, i).await;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Check that we can also get these inclusion proofs back.
    for i in 1..(insertion_batch_size + 1) {
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

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Insert not full batch to not trigger batch creation
    for i in 17..26 {
        test_insert_identity(&uri, &client, &mut ref_tree, &test_identities_ref, i).await;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Delete enough identities to trigger a deletion batch
    for i in 0..4 {
        test_delete_identity(&uri, &client, &mut ref_tree, &test_identities_ref, i, false).await;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    let mut ref_tree_2 = ref_tree.clone();
    // Insert not full batch to not trigger batch creation
    for i in 26..34 {
        test_insert_identity(&uri, &client, &mut ref_tree_2, &test_identities_ref, i).await;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Ensure that identities have been deleted
    for i in 0..4 {
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

    // Ensure that identities have been added
    for i in 17..26 {
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
