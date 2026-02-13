#![allow(clippy::needless_range_loop)]

mod common;

use common::prelude::*;

use crate::common::{test_delete_identity, test_insert_identity};

/// Polls by attempting to re-add an identity using v2 API.
/// The v2 API returns 409 Conflict for identities that already exist and are processed.
/// Returns true if the identity is confirmed processed (409 status), false otherwise.
async fn wait_for_identity_processed(
    uri: &str,
    client: &Client,
    identity: &Field,
    max_attempts: usize,
    sleep_duration: Duration,
) -> anyhow::Result<bool> {
    let identity_hex = format!("0x{:x}", identity);

    for attempt in 0..max_attempts {
        // Try to insert using v2 API - it returns 409 Conflict if identity already exists
        let response = client
            .post(format!("{}/v2/identities/{}", uri, identity_hex))
            .send()
            .await?;

        // 409 Conflict means the identity already exists and was processed
        if response.status() == StatusCode::CONFLICT {
            info!(
                "Identity confirmed processed after {} attempts",
                attempt + 1
            );
            return Ok(true);
        }

        if attempt < max_attempts - 1 {
            info!(
                "Identity not yet processed (status: {}), waiting {} seconds (attempt {}/{})",
                response.status(),
                sleep_duration.as_secs(),
                attempt + 1,
                max_attempts
            );
            tokio::time::sleep(sleep_duration).await;
        }
    }

    Ok(false)
}

/// Polls by attempting to re-add a deleted identity using v2 API.
/// The v2 API returns 410 Gone for deleted identities, which confirms the deletion was processed.
/// Returns true if the identity is confirmed deleted (410 status), false otherwise.
async fn wait_for_identity_deleted(
    uri: &str,
    client: &Client,
    identity: &Field,
    max_attempts: usize,
    sleep_duration: Duration,
) -> anyhow::Result<bool> {
    let identity_hex = format!("0x{:x}", identity);

    for attempt in 0..max_attempts {
        // Try to insert using v2 API - it returns 410 Gone if identity was deleted
        let response = client
            .post(format!("{}/v2/identities/{}", uri, identity_hex))
            .send()
            .await?;

        // 410 Gone means the identity was deleted and the deletion was processed
        if response.status() == StatusCode::GONE {
            info!("Identity confirmed deleted after {} attempts", attempt + 1);
            return Ok(true);
        }

        if attempt < max_attempts - 1 {
            info!(
                "Identity not yet deleted (status: {}), waiting {} seconds (attempt {}/{})",
                response.status(),
                sleep_duration.as_secs(),
                attempt + 1,
                max_attempts
            );
            tokio::time::sleep(sleep_duration).await;
        }
    }

    Ok(false)
}

/// This test verifies the following scenario:
/// 1. User adds an identity
/// 2. User removes the identity
/// 3. User adds the same identity again using API v3
///
/// Expected behavior: API v3 allows re-adding a deleted identity. The v3 endpoint
/// checks if the leaf index currently contains Hash::ZERO (deleted state). If so,
/// it permits the identity to be re-added at that same leaf index.
///
/// This differs from the older API versions which reject re-adding deleted identities.
///
/// Important: Deletions in reverse order can cause root duplicates, so we need
/// to insert additional identities between deletions to avoid this issue.
#[tokio::test]
async fn add_remove_readd_identity_onchain() -> anyhow::Result<()> {
    add_remove_readd_identity(false).await
}

#[tokio::test]
async fn add_remove_readd_identity_offchain() -> anyhow::Result<()> {
    add_remove_readd_identity(true).await
}

async fn add_remove_readd_identity(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting add-remove-readd identity integration test");

    let insertion_batch_size: usize = 3;
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
        .batch_insertion_timeout(Duration::from_secs(1))
        .batch_deletion_timeout(Duration::from_secs(1))
        .offchain_mode(offchain_mode_enabled)
        .build()?;

    let (_, app_handle, local_addr, shutdown) =
        spawn_app(config).await.expect("Failed to spawn app.");

    // Generate test identities
    // We need: 3 initial + 3 for second batch + 2 for final batch + 1 for re-add = 9 total
    // But we generate a few extra to be safe
    let test_identities = generate_test_identities(insertion_batch_size * 4);
    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    info!("Step 1: Insert initial batch of identities to trigger processing");
    // Insert first batch (indices 0, 1, 2)
    for i in 0..insertion_batch_size {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, i).await;
    }

    // Wait for the batch to be processed
    // Sleep for most of the expected time, then poll to check completion
    let processed = wait_for_identity_processed(
        &uri,
        &client,
        &identities_ref[0],
        30,
        Duration::from_secs(1),
    )
    .await?;
    assert!(processed, "First batch was not processed within timeout");

    info!("Step 2: Insert more identities to have enough for deletions");
    // Insert more identities (indices 3, 4, 5) to have more material to work with
    for i in insertion_batch_size..(insertion_batch_size + 3) {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, i).await;
    }

    // Wait for the second batch to be processed
    // Sleep for most of the expected time, then poll to check completion
    let processed = wait_for_identity_processed(
        &uri,
        &client,
        &identities_ref[insertion_batch_size],
        30,
        Duration::from_secs(1),
    )
    .await?;
    assert!(processed, "Second batch was not processed within timeout");

    info!("Step 3: Delete identity at index 1");
    // Delete identity at index 1
    test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, 1, false).await;

    info!("Step 4: Delete identity at index 0");
    // Delete identity at index 0 (the one we'll re-add later)
    test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, 0, false).await;

    info!("Step 5: Delete identity at index 2 to complete deletion batch");
    // Delete one more to complete a deletion batch (must be 3 for deletion_batch_size)
    test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, 2, false).await;

    // Wait for deletion batch to be processed
    // Use v2 API to check if the deleted identity returns 410 Gone
    // This directly confirms the deletion was processed
    let deleted = wait_for_identity_deleted(
        &uri,
        &client,
        &identities_ref[0], // Check the deleted identity itself
        40,
        Duration::from_secs(1),
    )
    .await?;
    assert!(deleted, "Deletion batch was not processed within timeout");

    info!("Step 8: Re-add the same identity that was deleted using API v3");
    // This is the key test: API v3 allows re-adding a deleted identity
    // The v3 API checks if the leaf index currently contains Hash::ZERO (deleted)
    // If so, it allows the identity to be re-added
    // IMPORTANT: The re-added identity will be inserted at the NEXT available leaf index,
    // not back at its original position. So the identity originally at index 0 will now
    // be at index 6 (after indices 0-5, where 3-5 are the new identities we added)
    let original_identity = identities_ref[0];
    let commitment_hex = format!("0x{:x}", original_identity);

    let response = client
        .post(format!("{}/v3/identities/{}", uri, commitment_hex))
        .send()
        .await
        .expect("Failed to send re-add request");

    // API v3 should accept re-adding a deleted identity with 202 Accepted
    assert_eq!(
        response.status(),
        StatusCode::ACCEPTED,
        "Expected 202 Accepted when re-adding deleted identity via v3 API, got: {:?}",
        response.status()
    );

    // The re-added identity will be at the next leaf index (6)
    // Current state: indices 0-2 are deleted (ZERO), indices 3-5 are occupied
    // So the re-added identity goes to index 6
    let readded_leaf_index = insertion_batch_size + 3; // = 6
    ref_tree.set(readded_leaf_index, original_identity);

    info!("Test completed successfully!");

    // Shutdown the app properly
    shutdown.shutdown();
    app_handle.await.unwrap();
    drop(db_container);
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }
    for (_, prover) in deletion_prover_map.into_iter() {
        prover.stop();
    }
    drop(micro_oz);

    Ok(())
}
