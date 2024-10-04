use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use sqlx::{Postgres, Transaction};
use tokio::sync::Notify;
use tokio::{select, time};
use tracing::info;

use crate::app::App;
use crate::database::query::DatabaseQuery as _;
use crate::database::types::DeletionEntry;
use crate::identity_tree::{Hash, TreeState, TreeVersionReadOps, UnprocessedStatus};
use crate::retry_tx;

// Because tree operations are single threaded (done one by one) we are running
// them from single task that determines which type of operations to run. It is
// done that way to reduce number of used mutexes and eliminate the risk of some
// tasks not being run at all as mutex is not preserving unlock order.
pub async fn modify_tree(
    app: Arc<App>,
    sync_tree_notify: Arc<Notify>,
    tree_synced_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    info!("Starting modify tree task.");

    let batch_deletion_timeout = chrono::Duration::from_std(app.config.app.batch_deletion_timeout)
        .context("Invalid batch deletion timeout duration")?;
    let min_batch_deletion_size = app.config.app.min_batch_deletion_size;

    let mut timer = time::interval(Duration::from_secs(5));

    loop {
        // We wait either for a timer tick or a full batch
        select! {
            _ = timer.tick() => {
                info!("Modify tree task woken due to timeout");
            }

            () = tree_synced_notify.notified() => {
                info!("Modify tree task woken due to tree synced event");
            },
        }

        let tree_state = app.tree_state()?;

        let tree_modified = retry_tx!(&app.database, tx, {
            do_modify_tree(
                &mut tx,
                batch_deletion_timeout,
                min_batch_deletion_size,
                tree_state,
            )
            .await
        })
        .await?;

        // It is very important to generate that event AFTER transaction is committed to
        // database. Otherwise, notified task may not see changes as transaction was not
        // committed yet.
        if tree_modified {
            sync_tree_notify.notify_one();
        }
    }
}

/// Looks for any pending changes to the tree. Returns true if there were any
/// changes applied to the tree.
async fn do_modify_tree(
    tx: &mut Transaction<'_, Postgres>,
    batch_deletion_timeout: chrono::Duration,
    min_batch_deletion_size: usize,
    tree_state: &TreeState,
) -> anyhow::Result<bool> {
    let deletions = tx.get_deletions().await?;

    // Deleting identities has precedence over inserting them.
    if should_run_deletion(
        tx,
        batch_deletion_timeout,
        min_batch_deletion_size,
        tree_state,
        &deletions,
    )
    .await?
    {
        run_deletions(tx, tree_state, deletions).await
    } else {
        run_insertions(tx, tree_state).await
    }
}

pub async fn should_run_deletion(
    tx: &mut Transaction<'_, Postgres>,
    batch_deletion_timeout: chrono::Duration,
    min_batch_deletion_size: usize,
    tree_state: &TreeState,
    deletions: &[DeletionEntry],
) -> anyhow::Result<bool> {
    let last_deletion_timestamp = tx.get_latest_deletion().await?.timestamp;

    if deletions.is_empty() {
        return Ok(false);
    }

    // If min batch size is not reached and batch deletion timeout not elapsed
    if deletions.len() < min_batch_deletion_size
        && Utc::now() - last_deletion_timestamp <= batch_deletion_timeout
    {
        return Ok(false);
    }

    // Now also check if the deletion batch could potentially create a duplicate
    // root batch
    if let Some(last_leaf_index) = tree_state.latest_tree().next_leaf().checked_sub(1) {
        let mut sorted_indices: Vec<usize> = deletions.iter().map(|v| v.leaf_index).collect();
        sorted_indices.sort();

        let indices_are_continuous = sorted_indices.windows(2).all(|w| w[1] == w[0] + 1);

        if indices_are_continuous && sorted_indices.last().unwrap() == &last_leaf_index {
            tracing::warn!(
                "Deletion batch could potentially create a duplicate root batch. Deletion batch \
                 will be postponed."
            );
            return Ok(false);
        }
    }

    Ok(true)
}

/// Run insertions and returns true if there were any changes to the tree.
pub async fn run_insertions(
    tx: &mut Transaction<'_, Postgres>,
    tree_state: &TreeState,
) -> anyhow::Result<bool> {
    let unprocessed = tx
        .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
        .await?;
    if unprocessed.is_empty() {
        return Ok(false);
    }

    let latest_tree = tree_state.latest_tree();

    // Filter out any identities that are already in the `identities` table
    let mut filtered_identities = vec![];
    for identity in unprocessed {
        if tx
            .get_identity_leaf_index(&identity.commitment)
            .await?
            .is_some()
        {
            tracing::warn!(?identity.commitment, "Duplicate identity");
            tx.remove_unprocessed_identity(&identity.commitment).await?;
        } else {
            filtered_identities.push(identity.commitment);
        }
    }

    let next_leaf = latest_tree.next_leaf();

    let next_db_index = tx.get_next_leaf_index().await?;

    assert_eq!(
        next_leaf, next_db_index,
        "Database and tree are out of sync. Next leaf index in tree is: {next_leaf}, in database: \
         {next_db_index}"
    );

    let mut pre_root = &latest_tree.get_root();
    let data = latest_tree
        .clone()
        .simulate_append_many(&filtered_identities);

    assert_eq!(
        data.len(),
        filtered_identities.len(),
        "Length mismatch when appending identities to tree"
    );

    for ((root, _proof, leaf_index), identity) in data.iter().zip(&filtered_identities) {
        tx.insert_pending_identity(*leaf_index, identity, root, pre_root)
            .await?;
        pre_root = root;

        tx.remove_unprocessed_identity(identity).await?;
    }

    Ok(true)
}

/// Run deletions and returns true if there were any changes to the tree.
pub async fn run_deletions(
    tx: &mut Transaction<'_, Postgres>,
    tree_state: &TreeState,
    mut deletions: Vec<DeletionEntry>,
) -> anyhow::Result<bool> {
    if deletions.is_empty() {
        return Ok(false);
    }

    // This sorting is very important. It ensures that we will create a unique root
    // after deletion. It mostly ensures that we won't delete things in reverse
    // order as they were added.
    deletions.sort_by(|v1, v2| v1.leaf_index.cmp(&v2.leaf_index));

    let (leaf_indices, previous_commitments): (Vec<usize>, Vec<Hash>) = deletions
        .iter()
        .map(|d| (d.leaf_index, d.commitment))
        .unzip();

    let mut pre_root = tree_state.latest_tree().get_root();
    // Delete the commitments at the target leaf indices in the latest tree,
    // generating the proof for each update
    let data = tree_state
        .latest_tree()
        .clone()
        .simulate_delete_many(&leaf_indices);

    assert_eq!(
        data.len(),
        leaf_indices.len(),
        "Length mismatch when appending identities to tree"
    );

    // Insert the new items into pending identities
    let items = data.into_iter().zip(leaf_indices);
    for ((root, _proof), leaf_index) in items {
        tx.insert_pending_identity(leaf_index, &Hash::ZERO, &root, &pre_root)
            .await?;
        pre_root = root;
    }

    // Remove the previous commitments from the deletions table
    tx.remove_deletions(&previous_commitments).await?;

    Ok(true)
}
