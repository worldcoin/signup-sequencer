mod common;

use anyhow::anyhow;
use common::prelude::*;

const IDLE_TIME: u64 = 7;

#[tokio::test]
async fn tree_restore_multiple_commitments_onchain() -> anyhow::Result<()> {
    tree_restore_multiple_commitments(false).await
}

#[tokio::test]
async fn tree_restore_multiple_commitments_offchain() -> anyhow::Result<()> {
    tree_restore_multiple_commitments(true).await
}

async fn tree_restore_multiple_commitments(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let batch_size: usize = 3;

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let docker = Cli::default();
    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) = spawn_deps(
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

    // temp dir will be deleted on drop call
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
        .offchain_mode(offchain_mode_enabled)
        .build()?;

    let (app, app_handle, local_addr, shutdown) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");

    let test_identities = generate_test_identities(3);
    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    let uri = "http://".to_owned() + &local_addr.to_string();
    let client = Client::new();

    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 0).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 1).await;
    test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 2).await;

    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    // Await for tree to be mined in app
    let tree_state = {
        let number_of_tries = 30;
        let mut tree_state = None;
        for _ in 0..number_of_tries {
            if app.tree_state()?.mined_tree().next_leaf() == 3 {
                tree_state = Some(app.tree_state()?);
                break;
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        tree_state.ok_or(anyhow!("Cannot get tree state"))?
    };

    assert_eq!(tree_state.mined_tree().next_leaf(), 3);

    // Shutdown the app and reset the mock shutdown, allowing us to test the
    // behaviour with saved data.
    info!("Stopping the app for testing purposes");
    shutdown.shutdown();
    app_handle.await.unwrap();

    let (app, app_handle, _, shutdown) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");

    let restored_tree_state = app.tree_state()?.clone();

    test_same_tree_states(tree_state, &restored_tree_state).await?;

    // Shutdown the app properly for the final time
    shutdown.shutdown();
    app_handle.await.unwrap();
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }

    Ok(())
}
