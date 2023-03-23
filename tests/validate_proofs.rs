mod common;

use common::prelude::*;

#[tokio::test]
async fn validate_proofs() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let mut ref_tree = PoseidonTree::new(SUPPORTED_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let (mock_chain, db_container, prover_mock) = spawn_deps(initial_root, 3).await?;

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
        "20",
    ])
    .expect("Failed to create options");
    options.server.server = Url::parse("http://127.0.0.1:0/").expect("Failed to parse URL");

    options.app.contracts.identity_manager_address = mock_chain.identity_manager_address;
    options.app.ethereum.read_options.confirmation_blocks_delay = 2;
    options.app.ethereum.read_options.ethereum_provider =
        Url::parse(&mock_chain.anvil.endpoint()).expect("Failed to parse ganache endpoint");
    options.app.ethereum.write_options.signing_key = mock_chain.private_key;

    options
        .app
        .prover
        .batch_insertion
        .batch_insertion_prover_url = prover_mock.url();

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

    // Shutdown the app properly for the final time
    shutdown();
    app.await.unwrap();
    prover_mock.stop();
    reset_shutdown();

    Ok(())
}
