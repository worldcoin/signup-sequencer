mod common;

use common::prelude::*;
use hyper::StatusCode;
use signup_sequencer::server::data::{
    IdentityHistoryEntryStatus, IdentityHistoryRequest, IdentityHistoryResponse,
};

use crate::common::test_recover_identity;

const HISTORY_POLLING_SLEEP: Duration = Duration::from_secs(5);
const MAX_HISTORY_POLLING_ATTEMPTS: usize = 24; // 2 minutes

#[tokio::test]
async fn identity_history() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let insertion_batch_size: usize = 8;
    let deletion_batch_size: usize = 3;

    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let (mock_chain, db_container, insertion_prover_map, deletion_prover_map, micro_oz) =
        spawn_deps(
            initial_root,
            &[insertion_batch_size],
            &[deletion_batch_size],
            DEFAULT_TREE_DEPTH as u8,
        )
        .await?;

    // Set the root history expirty to 30 seconds
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

    let config = TestConfigBuilder::new()
        .db_url(&db_url)
        .oz_api_url(&micro_oz.endpoint())
        .oz_address(micro_oz.address())
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .add_prover(mock_insertion_prover)
        .add_prover(mock_deletion_prover)
        .build()?;

    let (_, app_handle, local_addr) = spawn_app(config).await.expect("Failed to spawn app.");

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

    poll_history_until(
        &uri,
        &client,
        &identities_ref[0],
        |history| {
            history.history[0].kind.is_insertion()
                && history.history[0].status >= IdentityHistoryEntryStatus::Buffered
        },
        "Polling until first insertion is buffered",
    )
    .await?;

    poll_history_until(
        &uri,
        &client,
        &identities_ref[0],
        |history| {
            history.history[0].kind.is_insertion()
                && history.history[0].status >= IdentityHistoryEntryStatus::Pending
        },
        "Polling until first insertion is pending",
    )
    .await?;

    poll_history_until(
        &uri,
        &client,
        &identities_ref[0],
        |history| {
            history.history[0].kind.is_insertion()
                && history.history[0].status >= IdentityHistoryEntryStatus::Batched
        },
        "Polling until first insertion is batched",
    )
    .await?;

    poll_history_until(
        &uri,
        &client,
        &identities_ref[0],
        |history| {
            history.history[0].kind.is_insertion()
                && history.history[0].status >= IdentityHistoryEntryStatus::Bridged
        },
        "Polling until first insertion is mined/bridged",
    )
    .await?;

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

    let sample_recovery_identity = test_identities.len() - 1;

    tracing::info!("############ Deletion should be buffered ############");
    poll_history_until(
        &uri,
        &client,
        &identities_ref[0],
        |history| {
            history.history[0].kind.is_insertion()
                && history.history[0].status >= IdentityHistoryEntryStatus::Bridged
                && history.history[1].kind.is_deletion()
                && history.history[1].status >= IdentityHistoryEntryStatus::Buffered
        },
        "Polling until first deletion is buffered",
    )
    .await?;

    tracing::info!("############ Recovery should be queued ############");
    poll_history_until(
        &uri,
        &client,
        &identities_ref[sample_recovery_identity],
        |history| {
            !history.history.is_empty()
                && history.history[0].kind.is_insertion()
                && history.history[0].status >= IdentityHistoryEntryStatus::Queued
        },
        "Polling until first recovery is queued",
    )
    .await?;

    // Eventually the old identity will be deleted
    tracing::info!("############ Waiting for deletion to be mined/bridged ############");
    poll_history_until(
        &uri,
        &client,
        &identities_ref[0],
        |history| {
            history.history[0].kind.is_insertion()
                && history.history[0].status >= IdentityHistoryEntryStatus::Bridged
                && history.history[1].kind.is_deletion()
                && history.history[1].status >= IdentityHistoryEntryStatus::Bridged
        },
        "Polling until first deletion is mined/bridged",
    )
    .await?;

    // Sleep for root expiry
    tokio::time::sleep(Duration::from_secs(updated_root_history_expiry.as_u64())).await;

    // Insert enough identities to trigger an batch to be sent to the blockchain.
    for i in insertion_batch_size..insertion_batch_size * 2 {
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, i).await;
        next_leaf_index += 1;
    }

    tracing::info!("############ Final wait ############");
    poll_history_until(
        &uri,
        &client,
        &identities_ref[sample_recovery_identity],
        |history| {
            history.history[0].kind.is_insertion()
                && history.history[0].status >= IdentityHistoryEntryStatus::Bridged
        },
        "Polling until recovery is mined/bridged",
    )
    .await?;

    // Shutdown the app properly for the final time
    shutdown();
    app_handle.await.unwrap();
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }
    for (_, prover) in deletion_prover_map.into_iter() {
        prover.stop();
    }
    reset_shutdown();

    Ok(())
}

async fn poll_history_until(
    uri: &str,
    client: &Client<HttpConnector>,
    identity: &Field,
    predicate: impl Fn(&IdentityHistoryResponse) -> bool,
    label: &str,
) -> anyhow::Result<()> {
    for _ in 0..MAX_HISTORY_POLLING_ATTEMPTS {
        if let Some(history) = fetch_identity_history(uri, client, identity, label).await? {
            if predicate(&history) {
                return Ok(());
            } else {
                tracing::warn!("Label {label} - history {history:?} does not match predicate");
            }
        } else {
            tracing::warn!("No identity history for label {label}");
        }

        tokio::time::sleep(HISTORY_POLLING_SLEEP).await;
    }

    anyhow::bail!("Failed to fetch identity history within max attempts - {label}")
}

async fn fetch_identity_history(
    uri: &str,
    client: &Client<HttpConnector>,
    identity: &Field,
    label: &str,
) -> anyhow::Result<Option<IdentityHistoryResponse>> {
    let uri = format!("{uri}/identityHistory");
    let body = IdentityHistoryRequest {
        identity_commitment: *identity,
    };

    let req = Request::post(uri)
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&body)?))?;

    let mut response = client.request(req).await?;

    match response.status() {
        StatusCode::NOT_FOUND => return Ok(None),
        otherwise if otherwise.is_success() => {
            // continue
        }
        status => {
            anyhow::bail!("Failed to fetch identity history - status was {status} - label {label}")
        }
    }

    let body_bytes = hyper::body::to_bytes(response.body_mut()).await?;
    let body_bytes = body_bytes.to_vec();

    let body_string = String::from_utf8(body_bytes)?;

    let response: IdentityHistoryResponse = serde_json::from_str(&body_string)?;

    Ok(Some(response))
}
