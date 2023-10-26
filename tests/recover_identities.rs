mod common;

use common::prelude::*;
use signup_sequencer::identity_tree::{ProcessedStatus, UnprocessedStatus};

use crate::common::{test_inclusion_status, test_recover_identity};
const SUPPORTED_DEPTH: usize = 18;
const IDLE_TIME: u64 = 7;

#[tokio::test]
async fn recover_identities() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let insertion_batch_size: usize = 8;
    let deletion_batch_size: usize = 3;
    let batch_deletion_timeout_seconds: usize = 10;

    #[allow(clippy::cast_possible_truncation)]
    let tree_depth: u8 = SUPPORTED_DEPTH as u8;

    let mut ref_tree = PoseidonTree::new(SUPPORTED_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let (mock_chain, db_container, insertion_prover_map, deletion_prover_map, micro_oz) =
        spawn_deps(
            initial_root,
            &[insertion_batch_size],
            &[deletion_batch_size],
            tree_depth,
        )
        .await?;

    // Set the root history expirty to 15 seconds
    let updated_root_history_expiry = U256::from(30);
    mock_chain
        .identity_manager
        .method::<_, ()>("setRootHistoryExpiry", updated_root_history_expiry)?
        .send()
        .await?
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

    let mut next_leaf_index = 0;
    // Insert enough identities to trigger an batch to be sent to the blockchain.
    for i in 0..insertion_batch_size {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, i).await;

        next_leaf_index += 1;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;
    // Check that we can also get these inclusion proofs back.
    for i in 0..insertion_batch_size {
        test_inclusion_proof(&uri, &client, i, &ref_tree, &identities_ref[i], false).await;
    }

    // Insert enough recoveries to trigger a batch
    for i in 0..deletion_batch_size {
        // Delete the identity at i and replace it with an identity at the back of the
        //  test identities array
        // TODO: we should update to a much cleaner approach
        let recovery_leaf_index = test_identities.len() - i - 1;

        test_recover_identity(
            &uri,
            &client,
            &mut ref_tree,
            &identities_ref,
            i,
            identities_ref[recovery_leaf_index],
            next_leaf_index,
            false,
        )
        .await;

        next_leaf_index += 1;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME * 6)).await;

    // Ensure that identities have been deleted
    for i in 0..deletion_batch_size {
        let recovery_leaf_index = test_identities.len() - i - 1;

        test_inclusion_proof(&uri, &client, i, &ref_tree, &identities_ref[i], true).await;

        // Check that the replacement identity has not been inserted yet
        test_inclusion_status(
            &uri,
            &client,
            &identities_ref[recovery_leaf_index],
            UnprocessedStatus::New,
        )
        .await;
    }

    // Sleep for root expiry
    tokio::time::sleep(Duration::from_secs(updated_root_history_expiry.as_u64())).await;

    // Insert enough identities to trigger an batch to be sent to the blockchain.
    for i in insertion_batch_size..insertion_batch_size * 2 {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, i).await;
        next_leaf_index += 1;
    }

    tokio::time::sleep(Duration::from_secs(IDLE_TIME * 8)).await;

    // Check that the replacement identities have been inserted
    for i in 0..deletion_batch_size {
        let recovery_leaf_index = test_identities.len() - i - 1;

        // Check that the replacement identity has a mined status after an insertion
        // batch has completed
        test_inclusion_status(
            &uri,
            &client,
            &identities_ref[recovery_leaf_index],
            ProcessedStatus::Mined,
        )
        .await;
    }

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
