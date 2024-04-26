mod common;

use common::prelude::*;

#[tokio::test]
async fn validate_proofs() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let batch_timeout_seconds: u64 = 1;

    let batch_size = 3;

    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) =
        spawn_deps(initial_root, &[batch_size], &[], DEFAULT_TREE_DEPTH as u8).await?;

    let prover_mock = &insertion_prover_map[&batch_size];

    let identity_manager = mock_chain.identity_manager.clone();

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
        .add_prover(prover_mock)
        .build()?;

    let (_, app_handle, local_addr) = spawn_app(config).await.expect("Failed to spawn app.");

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    static IDENTITIES: Lazy<Vec<Identity>> = Lazy::new(|| {
        let mut s1 = *b"test_f0f0";
        let mut s2 = *b"test_f1f1";
        let mut s3 = *b"test_f2f2";
        vec![
            Identity::from_secret(&mut s1, None),
            Identity::from_secret(&mut s2, None),
            Identity::from_secret(&mut s3, None),
        ]
    });

    static TEST_LEAVES: Lazy<Vec<Field>> =
        Lazy::new(|| IDENTITIES.iter().map(|id| id.commitment()).collect());

    let signal_hash = hash_to_field(b"signal_hash");
    let external_nullifier_hash = hash_to_field(b"external_hash");

    // HAPPY PATH

    // generate identity
    let (merkle_proof, root) =
        test_insert_identity(&uri, &client, &mut ref_tree, &TEST_LEAVES, 0).await;

    // simulate client generating a proof
    let nullifier_hash = generate_nullifier_hash(&IDENTITIES[0], external_nullifier_hash);

    // Generates proof in the background
    let merkle_proof_for_task = merkle_proof.clone();
    let proof_task = tokio::task::spawn_blocking(move || {
        generate_proof(
            &IDENTITIES[0],
            &merkle_proof_for_task,
            external_nullifier_hash,
            signal_hash,
        )
        .unwrap()
    });

    tokio::time::sleep(Duration::from_secs(15 + batch_timeout_seconds)).await;

    let proof = proof_task.await.unwrap();

    test_verify_proof(
        &uri,
        &client,
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
        None,
    )
    .await;

    test_inclusion_proof(&uri, &client, 0, &ref_tree, &TEST_LEAVES[0], false).await;

    test_verify_proof_on_chain(
        &identity_manager,
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
    )
    .await
    .expect("Proof should verify correctly on chain.");

    // INVALID PROOF

    let invalid_nullifier_hash = generate_nullifier_hash(&IDENTITIES[1], external_nullifier_hash);

    test_verify_proof(
        &uri,
        &client,
        root,
        signal_hash,
        invalid_nullifier_hash,
        external_nullifier_hash,
        proof,
        Some("invalid semaphore proof"),
    )
    .await;

    test_verify_proof_on_chain(
        &identity_manager,
        root,
        signal_hash,
        invalid_nullifier_hash,
        external_nullifier_hash,
        proof,
    )
    .await
    .expect_err("Proof verified on chain when it shouldn't have.");

    // IDENTITY NOT IN TREE

    ref_tree.set(1, TEST_LEAVES[1]);

    // simulate client generating a proof
    let new_nullifier_hash = generate_nullifier_hash(&IDENTITIES[1], external_nullifier_hash);

    let new_proof = generate_proof(
        &IDENTITIES[1],
        &merkle_proof,
        external_nullifier_hash,
        signal_hash,
    )
    .unwrap();

    test_verify_proof(
        &uri,
        &client,
        root,
        signal_hash,
        new_nullifier_hash,
        external_nullifier_hash,
        new_proof,
        Some("invalid semaphore proof"),
    )
    .await;

    test_verify_proof_on_chain(
        &identity_manager,
        root,
        signal_hash,
        new_nullifier_hash,
        external_nullifier_hash,
        proof,
    )
    .await
    .expect_err("Proof verified on chain when it shouldn't have.");

    // UNKNOWN ROOT

    let new_root = ref_tree.root();

    test_verify_proof(
        &uri,
        &client,
        new_root,
        signal_hash,
        new_nullifier_hash,
        external_nullifier_hash,
        new_proof,
        Some("invalid root"),
    )
    .await;

    test_verify_proof_on_chain(
        &identity_manager,
        new_root,
        signal_hash,
        new_nullifier_hash,
        external_nullifier_hash,
        proof,
    )
    .await
    .expect_err("Proof verified on chain when it shouldn't have.");

    // Shutdown the app properly for the final time
    shutdown();
    app_handle.await.unwrap();
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
