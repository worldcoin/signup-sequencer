mod common;

use common::prelude::*;

use crate::common::test_verify_proof_with_age;

const SUPPORTED_DEPTH: usize = 20;

#[tokio::test]
async fn validate_proof_with_age() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let mut ref_tree = PoseidonTree::new(SUPPORTED_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let batch_timeout_seconds: u64 = 1;

    #[allow(clippy::cast_possible_truncation)]
    let tree_depth: u8 = SUPPORTED_DEPTH as u8;
    let batch_size = 3;

    let (mock_chain, db_container, prover_map, micro_oz) =
        spawn_deps(initial_root, &[batch_size], &vec![], tree_depth).await?;

    let prover_mock = &prover_map[&batch_size];

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
        "--time-between-scans-seconds",
        "1",
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

    let identities: Vec<Identity> = vec![
        Identity::from_secret(b"test_b1i1", None),
        Identity::from_secret(b"test_b1i2", None),
    ];

    let test_leaves: Vec<Field> = identities.iter().map(|id| id.commitment()).collect();

    let signal_hash = hash_to_field(b"signal_hash");
    let external_nullifier_hash = hash_to_field(b"external_hash");

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

    // Test proof which is new enough
    test_verify_proof_with_age(
        &uri,
        &client,
        root,
        signal_hash,
        nullifier_hash,
        external_nullifier_hash,
        proof,
        (2 * sleep_duration_seconds) as i64 + 5, // 5 seconds margin
        None,
    )
    .await;

    // Shutdown the app properly for the final time
    shutdown();
    app.await.unwrap();
    for (_, prover) in prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
