mod common;

use common::prelude::*;

use crate::common::{test_add_batch_size, test_remove_batch_size};

const IDLE_TIME: u64 = 15;

#[tokio::test]
async fn dynamic_batch_sizes_onchain() -> anyhow::Result<()> {
    dynamic_batch_sizes(false).await
}

#[tokio::test]
async fn dynamic_batch_sizes_offchain() -> anyhow::Result<()> {
    dynamic_batch_sizes(true).await
}

async fn dynamic_batch_sizes(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let first_batch_size: usize = 3;
    let second_batch_size: usize = 2;

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let docker = Cli::default();
    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) = spawn_deps(
        initial_root,
        &[first_batch_size, second_batch_size],
        &[],
        DEFAULT_TREE_DEPTH as u8,
        &docker,
    )
    .await?;

    let first_prover = &insertion_prover_map[&first_batch_size];
    let second_prover = &insertion_prover_map[&second_batch_size];

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
        // We initially spawn the sequencer with only the first prover
        .add_prover(first_prover)
        .offchain_mode(offchain_mode_enabled)
        .build()?;

    let (_, app_handle, local_addr, shutdown) =
        spawn_app(config).await.expect("Failed to spawn app.");

    let test_identities = generate_test_identities(first_batch_size * 5);
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

    // wait at leat 5 seconds before checking proof
    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Check that we can get their inclusion proofs back.
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        0,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[0], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
        offchain_mode_enabled,
    )
    .await;
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        1,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[1], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
        offchain_mode_enabled,
    )
    .await;
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        2,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[2], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
        offchain_mode_enabled,
    )
    .await;

    test_add_batch_size(
        &uri,
        second_prover.url(),
        second_batch_size as u64,
        second_prover.prover_type(),
        &client,
    )
    .await
    .expect("Failed to add batch size.");

    // Query for the available provers.
    let batch_sizes_uri = format!("{uri}/listBatchSizes");
    let batch_sizes = client
        .get(batch_sizes_uri)
        .send()
        .await
        .expect("Failed to execute get request");

    let batch_sizes_bytes = batch_sizes
        .bytes()
        .await
        .expect("Failed to get response bytes");

    let batch_size_str = String::from_utf8(batch_sizes_bytes.into_iter().collect())
        .expect("Failed to decode response");
    let batch_size_json =
        serde_json::from_str::<serde_json::Value>(&batch_size_str).expect("JSON wasn't decoded");
    assert_eq!(
        batch_size_json,
        json!([
            {
                "url": second_prover.url() + "/",
                "timeout_s": 3,
                "batch_size": second_batch_size,
                "prover_type": "insertion",
            },
            {
                "url": first_prover.url() + "/",
                "timeout_s": 30,
                "batch_size": first_batch_size,
                "prover_type": "insertion",

            }
        ])
    );

    // Insert enough identities to trigger the lower batch size.
    first_prover.set_availability(false).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 3).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 4).await;

    // Force a timeout by resetting the tokio runtime's timer.
    tokio::time::pause();
    tokio::time::resume();

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Check that we can also get these inclusion proofs back.
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        3,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[3], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
        offchain_mode_enabled,
    )
    .await;
    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        4,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[4], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
        offchain_mode_enabled,
    )
    .await;

    // Now if we remove the original prover, things should still work.
    test_remove_batch_size(
        &uri,
        first_batch_size as u64,
        &client,
        first_prover.prover_type(),
        false,
    )
    .await?;

    // We should be able to insert less than a full batch successfully.
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 5).await;
    tokio::time::pause();
    tokio::time::resume();

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        5,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[5], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
        offchain_mode_enabled,
    )
    .await;

    // We should be unable to remove _all_ of the provers, however.
    test_remove_batch_size(
        &uri,
        second_batch_size as u64,
        &client,
        second_prover.prover_type(),
        true,
    )
    .await?;

    // So we should still be able to run a batch.
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 6).await;
    tokio::time::pause();
    tokio::time::resume();

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    test_inclusion_proof(
        &mock_chain,
        &uri,
        &client,
        6,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[6], 16)
            .expect("Failed to parse Hash from test leaf 0"),
        false,
        offchain_mode_enabled,
    )
    .await;

    // Shutdown the app properly for the final time
    shutdown.shutdown();
    app_handle.await.unwrap();
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }

    Ok(())
}
