use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use sqlx::{Postgres, Transaction};
use tokio::sync::watch::Receiver;
use tokio::sync::{MutexGuard, Notify};
use tokio::time::MissedTickBehavior;
use tokio::{select, time};
use tracing::{info, warn};

use crate::app::App;
use crate::database::methods::DbMethods;
use crate::database::types::DeletionEntry;
use crate::identity_tree::{Hash, TreeState, TreeVersionReadOps};
use crate::retry_tx;

// Because tree operations are single threaded (done one by one) we are running
// them from single task that determines which type of operations to run. It is
// done that way to reduce number of used mutexes and eliminate the risk of some
// tasks not being run at all as mutex is not preserving unlock order.
pub async fn modify_tree(
    app: Arc<App>,
    sync_tree_notify: Arc<Notify>,
    mut tree_synced_rx: Receiver<()>,
) -> anyhow::Result<()> {
    info!("Starting modify tree task.");

    let batch_deletion_timeout = chrono::Duration::from_std(app.config.app.batch_deletion_timeout)
        .context("Invalid batch deletion timeout duration")?;
    let min_batch_deletion_size = app.config.app.min_batch_deletion_size;

    let mut timer = time::interval(Duration::from_secs(5));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        // We wait either for a timer tick or a event that tree was synchronized
        select! {
            _ = timer.tick() => {
                info!("Modify tree task woken due to timeout");
            }

            _ = tree_synced_rx.changed() => {
                info!("Modify tree task woken due to tree synced event");
            },
        }

        let request_sync = retry_tx!(&app.database, tx, {
            let tree_state = app.tree_state().await?;

            let latest_tree = tree_state.get_latest_tree();
            let next_leaf_tree = latest_tree.next_leaf();
            let next_leaf_db = tx.get_next_leaf_index().await?;

            if next_leaf_tree != next_leaf_db {
                warn!(
                    "Database and tree are out of sync. Next leaf index in tree is: {}, in \
                     database: {}",
                    next_leaf_tree, next_leaf_db
                );
                return Ok(true);
            }

            do_modify_tree(
                &mut tx,
                batch_deletion_timeout,
                min_batch_deletion_size,
                &tree_state,
            )
            .await
        })
        .await?;

        info!(request_sync, "Modify tree task finished");

        // It is very important to generate that event AFTER transaction is committed to
        // database. Otherwise, notified task may not see changes as transaction was not
        // committed yet.
        if request_sync {
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
    tree_state: &MutexGuard<'_, TreeState>,
) -> anyhow::Result<bool> {
    let deletions = get_deletions(tx, batch_deletion_timeout, min_batch_deletion_size).await?;

    // Deleting identities has precedence over inserting them.
    // If deletions fail (e.g., would create duplicate roots), fall through to insertions.
    if !deletions.is_empty() && run_deletions(tx, tree_state, deletions).await? {
        Ok(true)
    } else {
        run_insertions(tx, tree_state).await
    }
}

pub async fn get_deletions(
    tx: &mut Transaction<'_, Postgres>,
    batch_deletion_timeout: chrono::Duration,
    min_batch_deletion_size: usize,
) -> anyhow::Result<Vec<DeletionEntry>> {
    let deletions = tx.get_deletions().await?;

    if deletions.is_empty() {
        return Ok(Vec::new());
    }

    // If the minimum deletions batch size is not reached and the deletion time
    // interval has not elapsed then we can skip
    if deletions.len() < min_batch_deletion_size {
        let last_deletion_timestamp = tx.get_latest_deletion().await?.timestamp;
        if Utc::now() - last_deletion_timestamp <= batch_deletion_timeout {
            return Ok(Vec::new());
        }
    }

    // Dedup deletion entries
    let deletions = deletions.into_iter().collect::<HashSet<DeletionEntry>>();
    let deletions = deletions.into_iter().collect::<Vec<DeletionEntry>>();

    Ok(deletions)
}

/// Run insertions and returns true if there were any changes to the tree.
pub async fn run_insertions(
    tx: &mut Transaction<'_, Postgres>,
    tree_state: &MutexGuard<'_, TreeState>,
) -> anyhow::Result<bool> {
    let unprocessed = tx.get_unprocessed_identities().await?;
    if unprocessed.is_empty() {
        return Ok(false);
    }

    let latest_tree = tree_state.latest_tree();

    let mut pre_root = &latest_tree.get_root();
    let data = latest_tree.clone().simulate_append_many(
        &unprocessed
            .iter()
            .map(|v| v.commitment)
            .collect::<Vec<Hash>>(),
    );

    assert_eq!(
        data.len(),
        unprocessed.len(),
        "Length mismatch when appending identities to tree"
    );

    for ((root, _proof, leaf_index), identity) in data.iter().zip(&unprocessed) {
        tx.insert_pending_identity(
            *leaf_index,
            &identity.commitment,
            identity.created_at,
            root,
            pre_root,
        )
        .await?;

        pre_root = root;
    }

    tx.trim_unprocessed().await?;

    Ok(true)
}

/// Run deletions and returns true if there were any changes to the tree.
pub async fn run_deletions(
    tx: &mut Transaction<'_, Postgres>,
    tree_state: &MutexGuard<'_, TreeState>,
    deletions: Vec<DeletionEntry>,
) -> anyhow::Result<bool> {
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

    // Verify that none of the simulated deletion roots already exist in the database.
    // This prevents duplicate roots which could occur in various deletion patterns.
    // If any duplicate is found, we abort deletions and return Ok(false) to signal
    // the caller to fall through to insertions instead.
    for (root, _proof) in &data {
        if let Some(_existing) = tx.get_root_state(root).await? {
            warn!(
                "Deletion batch would create duplicate root. Skipping deletions to allow \
                 insertions instead"
            );
            return Ok(false);
        }
    }

    // Insert the new items into pending identities
    let items = data.into_iter().zip(deletions);
    for ((root, _proof), d) in items {
        tx.insert_pending_identity(d.leaf_index, &Hash::ZERO, d.created_at, &root, &pre_root)
            .await?;
        pre_root = root;
    }

    // Remove the previous commitments from the deletions table
    tx.remove_deletions(&previous_commitments).await?;

    Ok(true)
}
