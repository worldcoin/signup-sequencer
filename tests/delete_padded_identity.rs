#![allow(clippy::needless_range_loop)]

mod common;

use common::prelude::*;

use crate::common::test_delete_identity;

const SUPPORTED_DEPTH: u8 = 18;
const IDLE_TIME: u64 = 7;

#[tokio::test]
async fn delete_padded_identity() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let insertion_batch_size: usize = 8;
    let deletion_batch_size: usize = 3;
    let batch_deletion_timeout_seconds: usize = 10;

    #[allow(clippy::cast_possible_truncation)]
    let tree_depth: u8 = SUPPORTED_DEPTH;

    let mut ref_tree = PoseidonTree::new((SUPPORTED_DEPTH + 1).into(), ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let (mock_chain, db_container, insertion_prover_map, deletion_prover_map, micro_oz) =
        spawn_deps(
            initial_root,
            &[insertion_batch_size],
            &[deletion_batch_size],
            tree_depth,
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
        &format!(
            "[{}, {}]",
            mock_insertion_prover.arg_string_single(),
            mock_deletion_prover.arg_string_single()
        ),
        "--batch-timeout-seconds",
        "10",
        "--batch-deletion-timeout-seconds",
        &format!("{batch_deletion_timeout_seconds}"),
        "--min-batch-deletion-size",
        &format!("{deletion_batch_size}"),
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
        "--dense-tree-mmap-file",
        temp_dir.path().join("testfile").to_str().unwrap(),
    ])
    .context("Failed to create options")?;

    options.server.server = Url::parse("http://127.0.0.1:0/").expect("Failed to parse URL");

    options.app.contracts.identity_manager_address = mock_chain.identity_manager.address();
    options.app.ethereum.ethereum_provider = Url::parse(&mock_chain.anvil.endpoint()).expect(
        "
    Failed to parse Anvil url",
    );

    let (app, local_addr) = spawn_app(options.clone())
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
            &uri,
            &client,
            i,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[i], 16)
                .expect("Failed to parse Hash from test leaf"),
            false,
        )
        .await;
    }

    // delete only the first and second identities
    test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, 0, false).await;
    test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, 1, false).await;

    tokio::time::sleep(Duration::from_secs(
        batch_deletion_timeout_seconds as u64 * 3,
    ))
    .await;

    // make sure that identity 3 wasn't deleted
    test_inclusion_proof(
        &uri,
        &client,
        2,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[2], 16)
            .expect("Failed to parse Hash from test leaf"),
        false,
    )
    .await;

    // Ensure that the first and second identities were deleted
    test_inclusion_proof(
        &uri,
        &client,
        0,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[0], 16)
            .expect("Failed to parse Hash from test leaf"),
        true,
    )
    .await;
    test_inclusion_proof(
        &uri,
        &client,
        1,
        &ref_tree,
        &Hash::from_str_radix(&test_identities[1], 16)
            .expect("Failed to parse Hash from test leaf"),
        true,
    )
    .await;

    // Expect failure when deleting an identity that has already been deleted
    test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, 0, true).await;

    // Expect failure when deleting an identity that can not be found
    test_delete_identity(&uri, &client, &mut ref_tree, &identities_ref, 12, true).await;

    // Shutdown the app properly for the final time
    shutdown();
    app.await.unwrap();
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }
    for (_, prover) in deletion_prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}
