mod common;

use common::prelude::*;

const SUPPORTED_DEPTH: usize = 20;

#[tokio::test]
async fn validate_proofs() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let mut ref_tree = PoseidonTree::new(SUPPORTED_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let batch_timeout_seconds: u64 = 1;

    #[allow(clippy::cast_possible_truncation)]
    let tree_depth: u8 = SUPPORTED_DEPTH as u8;
    let batch_size = 3;

    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) =
        spawn_deps(initial_root, &[batch_size], &vec![], tree_depth).await?;

    let prover_mock = &insertion_prover_map[&batch_size];

    let identity_manager = mock_chain.identity_manager.clone();

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
    .expect("Failed to create options");
    options.server.server = Url::parse("http://127.0.0.1:0/").expect("Failed to parse URL");

    options.app.contracts.identity_manager_address = mock_chain.identity_manager.address();
    options.app.ethereum.ethereum_provider =
        Url::parse(&mock_chain.anvil.endpoint()).expect("Failed to parse ganache endpoint");

    let (app, local_addr) = spawn_app(options.clone())
        .await
        .expect("Failed to spawn app.");

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    static IDENTITIES: Lazy<Vec<Identity>> = Lazy::new(|| {
        vec![
            Identity::from_secret(b"test_f0f0", None),
            Identity::from_secret(b"test_f1f1", None),
            Identity::from_secret(b"test_f2f2", None),
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

    tokio::time::sleep(Duration::from_secs(5 + batch_timeout_seconds)).await;
    // simulate client generating a proof
    let nullifier_hash = generate_nullifier_hash(&IDENTITIES[0], external_nullifier_hash);

    let proof = generate_proof(
        &IDENTITIES[0],
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
    app.await.unwrap();
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
