mod common;

use std::time::Instant;

use common::prelude::*;

use crate::common::test_verify_proof_with_age;

#[tokio::test]
async fn validate_proof_with_age_onchain() -> anyhow::Result<()> {
    validate_proof_with_age(false).await
}

#[tokio::test]
async fn validate_proof_with_age_offchain() -> anyhow::Result<()> {
    validate_proof_with_age(true).await
}

async fn validate_proof_with_age(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let batch_timeout_seconds: u64 = 1;

    #[allow(clippy::cast_possible_truncation)]
    let batch_size = 3;

    let docker = Cli::default();
    let (mock_chain, db_container, insertion_prover_map, _deletion_prover_map, micro_oz) =
        spawn_deps(
            initial_root,
            &[batch_size],
            &[],
            DEFAULT_TREE_DEPTH as u8,
            &docker,
        )
        .await?;

    let prover_mock = &insertion_prover_map[&batch_size];

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
        .batch_insertion_timeout(Duration::from_secs(batch_timeout_seconds))
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .add_prover(prover_mock)
        .offchain_mode(offchain_mode_enabled)
        .build()?;

    let (_, app_handle, local_addr, shutdown) =
        spawn_app(config).await.expect("Failed to spawn app.");

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    let mut s1 = *b"test_f0f0";
    let mut s2 = *b"test_f1f1";

    let identities: Vec<Identity> = vec![
        Identity::from_secret(&mut s1, None),
        Identity::from_secret(&mut s2, None),
    ];

    let test_leaves: Vec<Field> = identities.iter().map(|id| id.commitment()).collect();

    let signal_hash = hash_to_field(b"signal_hash");
    let external_nullifier_hash = hash_to_field(b"external_hash");

    let time_of_identity_insertion = Instant::now();

    // Insert only the 1st identity
    let (merkle_proof, root) =
        test_insert_identity(&uri, &client, &mut ref_tree, &test_leaves, 0).await;

    let sleep_duration_seconds = 15 + batch_timeout_seconds;

    tokio::time::sleep(Duration::from_secs(sleep_duration_seconds)).await;

    // simulate client generating a proof
    let nullifier_hash = generate_nullifier_hash(&identities[0], external_nullifier_hash);

    let proof = generate_proof(
        &identities[0],
        &merkle_proof,
        external_nullifier_hash,
        signal_hash,
    )
    .unwrap();

    // Wait so the proof is at least 2 seconds old
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Proof will be older than 2 seconds, but it's the latest root
    test_verify_proof_with_age(
        &uri,
        &client,
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
        2,
        None,
    )
    .await;

    // Insert the 2nd identity to produce new root
    test_insert_identity(&uri, &client, &mut ref_tree, &test_leaves, 1).await;

    // Wait for batch
    tokio::time::sleep(Duration::from_secs(sleep_duration_seconds)).await;

    // Now the old proof root is too old (definitely older than 2 seconds)
    test_verify_proof_with_age(
        &uri,
        &client,
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
        2,
        Some("Root provided in semaphore proof is too old."),
    )
    .await;

    let max_age_of_proof = (Instant::now() - time_of_identity_insertion).as_secs();
    assert!(
        max_age_of_proof > sleep_duration_seconds * 2,
        "Proof age should be at least 2 batch times"
    );

    // Test proof which is new enough
    test_verify_proof_with_age(
        &uri,
        &client,
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
        max_age_of_proof as i64,
        None,
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
