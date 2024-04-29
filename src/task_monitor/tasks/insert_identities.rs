use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::time::sleep;
use tracing::instrument;

use crate::app::App;
use crate::database::types::{BatchType, Commitments, LeafIndexes, UnprocessedCommitment};
use crate::database::{Database, DatabaseExt};
use crate::identity_tree::{Latest, TreeVersion, TreeVersionReadOps, UnprocessedStatus};

// todo(piotrh): ensure things are batched properly to save $$$ when executed
// on, add check timeour chain
pub async fn insert_identities(
    app: Arc<App>,
    pending_insertions_mutex: Arc<Mutex<()>>,
    wake_up_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    ensure_batch_chain_initialized(&app).await?;

    let batch_size = app.identity_manager.max_insertion_batch_size().await;

    loop {
        // get commits from database
        let unprocessed = app
            .database
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New, batch_size)
            .await?;
        if unprocessed.is_empty() {
            sleep(Duration::from_secs(5)).await;
            continue;
        }

        insert_identities_batch(
            &app.database,
            app.tree_state()?.latest_tree(),
            unprocessed,
            &pending_insertions_mutex,
        )
        .await?;

        // Notify the identity processing task, that there are new identities
        wake_up_notify.notify_one();
    }
}

async fn ensure_batch_chain_initialized(app: &Arc<App>) -> anyhow::Result<()> {
    let batch_head = app.database.get_batch_head().await?;
    if batch_head.is_none() {
        app.database
            .insert_new_batch_head(
                &app.tree_state()?.latest_tree().get_root(),
                BatchType::Insertion,
                &Commitments(vec![]),
                &LeafIndexes(vec![]),
            )
            .await?;
    }
    Ok(())
}

#[instrument(level = "info", skip_all)]
async fn insert_identities_batch(
    database: &Database,
    latest_tree: &TreeVersion<Latest>,
    identities: Vec<UnprocessedCommitment>,
    pending_insertions_mutex: &Mutex<()>,
) -> anyhow::Result<()> {
    // Filter out any identities that are already in the `identities` table
    let mut filtered_identities = vec![];
    for identity in identities {
        if database
            .get_identity_leaf_index(&identity.commitment)
            .await?
            .is_some()
        {
            tracing::warn!(?identity.commitment, "Duplicate identity");
            database
                .remove_unprocessed_identity(&identity.commitment)
                .await?;
        } else {
            filtered_identities.push(identity.commitment);
        }
    }

    let _guard = pending_insertions_mutex.lock().await;

    let next_db_index = database.get_next_leaf_index().await?;
    let next_leaf = latest_tree.next_leaf();
    let prev_root = latest_tree.get_root();

    assert_eq!(
        next_leaf, next_db_index,
        "Database and tree are out of sync. Next leaf index in tree is: {next_leaf}, in database: \
         {next_db_index}"
    );

    let (data, _) = latest_tree.append_many_as_derived(&filtered_identities);
    let next_root = data
        .last()
        .map(|(root, ..)| root.clone())
        .expect("should be created at least one");

    assert_eq!(
        data.len(),
        filtered_identities.len(),
        "Length mismatch when appending identities to tree"
    );

    let items: Vec<_> = data.into_iter().zip(filtered_identities.clone()).collect();

    let mut tx = database.pool.begin().await?;

    for ((root, _proof, leaf_index), identity) in items.iter() {
        tx.insert_pending_identity(*leaf_index, identity, root)
            .await?;

        tx.remove_unprocessed_identity(identity).await?;
    }

    tx.insert_new_batch(
        &next_root,
        &prev_root,
        BatchType::Insertion,
        &Commitments(items.iter().map(|(_, commitment)| *commitment).collect()),
        &LeafIndexes(
            items
                .iter()
                .map(|((_, _, leaf_index), _)| *leaf_index)
                .collect(),
        ),
    )
    .await?;

    tx.commit().await?;

    // todo(piotrh): ensure if we can or not do it here
    // _ = latest_tree.append_many(&filtered_identities);

    Ok(())
}
