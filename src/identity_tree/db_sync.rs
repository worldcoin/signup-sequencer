use crate::database::methods::DbMethods;
use crate::identity_tree::{
    Canonical, Intermediate, Latest, ProcessedStatus, ReversibleVersion, TreeState, TreeUpdate,
    TreeVersion, TreeVersionReadOps, TreeWithNextVersion,
};
use anyhow::bail;
use sqlx::{Postgres, Transaction};
use std::cmp::Ordering;
use tokio::sync::MutexGuard;
use tracing::debug;

pub struct SyncTreeResult {
    pub latest_tree_updates: Vec<TreeUpdate>,
}

/// A point-in-time snapshot of the last sequence IDs for all four tree
/// versions. Captured while the `TreeState` mutex is held, then used as
/// anchors for the subsequent DB-only read phase.
#[derive(Copy, Clone, Debug)]
pub struct TreeStateSnapshot {
    pub mined_last_seq: usize,
    pub processed_last_seq: usize,
    pub batching_last_seq: usize,
    pub latest_last_seq: usize,
}

impl TreeStateSnapshot {
    pub fn from_tree_state(tree_state: &TreeState) -> Self {
        Self {
            mined_last_seq: tree_state.mined_tree().get_last_sequence_id(),
            processed_last_seq: tree_state.processed_tree().get_last_sequence_id(),
            batching_last_seq: tree_state.batching_tree().get_last_sequence_id(),
            latest_last_seq: tree_state.latest_tree().get_last_sequence_id(),
        }
    }
}

/// Everything the DB read phase determined about what needs to change in each
/// tree. No `MutexGuard` is stored here — this is safe to keep across an
/// `.await` boundary.
pub struct SyncPlan {
    /// The snapshot that was used to build this plan.
    pub snapshot: TreeStateSnapshot,
    /// Row-level updates to append to the latest tree (forward-only).
    pub latest_tree_updates: Vec<TreeUpdate>,
    /// Target tree-update (post-root + seq-id) for each tree tier. `None`
    /// means "no data in DB yet — leave the tree alone".
    pub latest_target: Option<TreeUpdate>,
    pub batching_target: Option<TreeUpdate>,
    pub processed_target: Option<TreeUpdate>,
    pub mined_target: Option<TreeUpdate>,
}

/// Order of operations in sync tree is very important as it ensures we can
/// apply new updates or rewind them properly.
pub async fn sync_tree(
    tx: &mut Transaction<'_, Postgres>,
    tree_state: &MutexGuard<'_, TreeState>,
) -> anyhow::Result<SyncTreeResult> {
    let mined_tree = tree_state.mined_tree();
    let processed_tree = tree_state.processed_tree();
    let batching_tree = tree_state.batching_tree();
    let latest_tree = tree_state.latest_tree();

    let latest_mined_tree_update = tx
        .get_latest_tree_update_by_statuses(vec![ProcessedStatus::Mined])
        .await?;

    // First check if mined tree needs to be rolled back. If so then we must
    // panic to quit to rebuild the tree on startup. This is a time-consuming
    // operation.
    assert!(
        latest_mined_tree_update
            .clone()
            .map(|u| u.sequence_id)
            .unwrap_or(0)
            >= mined_tree.get_last_sequence_id(),
        "Mined tree needs to be rolled back."
    );

    // Get all other roots from database
    let latest_processed_tree_update = tx
        .get_latest_tree_update_by_statuses(vec![
            ProcessedStatus::Processed,
            ProcessedStatus::Mined,
        ])
        .await?;

    let latest_pending_tree_update = tx
        .get_latest_tree_update_by_statuses(vec![
            ProcessedStatus::Pending,
            ProcessedStatus::Processed,
            ProcessedStatus::Mined,
        ])
        .await?;

    let latest_batch = tx.get_latest_batch().await?;
    let latest_batching_tree_update = if let Some(latest_batch) = latest_batch {
        tx.get_tree_update_by_root(&latest_batch.next_root).await?
    } else {
        latest_processed_tree_update.clone()
    };

    // And then update trees
    let latest_tree_updates =
        update_latest_tree(tx, latest_tree, &latest_pending_tree_update, || {
            update_batching_tree(batching_tree, &latest_batching_tree_update, || {
                update_processed_tree(processed_tree, &latest_processed_tree_update, || {
                    update_mined_tree(mined_tree, &latest_mined_tree_update)
                })
            })
        })
        .await?;

    Ok(SyncTreeResult {
        latest_tree_updates,
    })
}

/// Issue all DB reads needed to produce a [`SyncPlan`]. The `TreeState` mutex is **not** held
/// during this call.
pub async fn build_sync_plan(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &TreeStateSnapshot,
) -> anyhow::Result<SyncPlan> {
    let mined_target = tx
        .get_latest_tree_update_by_statuses(vec![ProcessedStatus::Mined])
        .await?;

    // Preserve the startup invariant: mined tree must never need a rollback.
    assert!(
        mined_target.as_ref().map(|u| u.sequence_id).unwrap_or(0) >= snapshot.mined_last_seq,
        "Mined tree needs to be rolled back."
    );

    let processed_target = tx
        .get_latest_tree_update_by_statuses(vec![
            ProcessedStatus::Processed,
            ProcessedStatus::Mined,
        ])
        .await?;

    let latest_target = tx
        .get_latest_tree_update_by_statuses(vec![
            ProcessedStatus::Pending,
            ProcessedStatus::Processed,
            ProcessedStatus::Mined,
        ])
        .await?;

    let latest_batch = tx.get_latest_batch().await?;
    let batching_target = if let Some(latest_batch) = latest_batch {
        tx.get_tree_update_by_root(&latest_batch.next_root).await?
    } else {
        processed_target.clone()
    };

    // Eagerly fetch the incremental updates for the latest tree so that the
    // apply phase can work without any DB access.
    let latest_tree_updates = match &latest_target {
        Some(target) if target.sequence_id > snapshot.latest_last_seq => {
            tx.get_tree_updates_after_id(snapshot.latest_last_seq)
                .await?
        }
        _ => vec![],
    };

    Ok(SyncPlan {
        snapshot: *snapshot,
        latest_tree_updates,
        latest_target,
        batching_target,
        processed_target,
        mined_target,
    })
}

impl SyncPlan {
    /// Apply this plan to the in-memory trees. The `TreeState` mutex **must**
    /// be held by the caller for the duration of this call.
    ///
    /// If another writer advanced the trees between phase 1 and phase 3,
    /// already-applied `latest_tree_updates` are filtered out before applying.
    /// Rewind and error semantics follow the same rules as [`sync_tree`]:
    /// non-mined trees rewind in-memory; the mined tree returns an error.
    pub fn apply(self, tree_state: &MutexGuard<'_, TreeState>) -> anyhow::Result<SyncTreeResult> {
        let plan = self;
        let mined_tree = tree_state.mined_tree();
        let processed_tree = tree_state.processed_tree();
        let batching_tree = tree_state.batching_tree();
        let latest_tree = tree_state.latest_tree();

        let latest_seq = latest_tree.get_last_sequence_id();

        // Filter out updates already applied by another writer between phases.
        let latest_tree_updates: Vec<_> = plan
            .latest_tree_updates
            .into_iter()
            .filter(|u| u.sequence_id > latest_seq)
            .collect();

        let tree_updates = apply_latest_tree(
            latest_tree,
            &plan.latest_target,
            latest_tree_updates,
            || {
                apply_batching_tree(batching_tree, &plan.batching_target, || {
                    apply_processed_tree(processed_tree, &plan.processed_target, || {
                        apply_mined_tree(mined_tree, &plan.mined_target)
                    })
                })
            },
        )?;

        Ok(SyncTreeResult {
            latest_tree_updates: tree_updates,
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Private helpers used by both sync_tree (original) and SyncPlan::apply
// ──────────────────────────────────────────────────────────────────────────────

async fn update_latest_tree<F: Fn() -> anyhow::Result<()>>(
    tx: &mut Transaction<'_, Postgres>,
    latest_tree: &TreeVersion<Latest>,
    latest_tree_update: &Option<TreeUpdate>,
    update_batching_tree: F,
) -> anyhow::Result<Vec<TreeUpdate>> {
    let Some(latest_tree_update) = latest_tree_update else {
        debug!("No latest tree update.");
        update_batching_tree()?;

        return Ok(vec![]);
    };

    let current_sequence_id = latest_tree.get_last_sequence_id();
    let new_sequence_id = latest_tree_update.sequence_id;

    let tree_updates = match new_sequence_id.cmp(&current_sequence_id) {
        Ordering::Greater => {
            debug!("Applying latest tree updates up to {}", new_sequence_id);
            let tree_updates = tx
                .get_tree_updates_after_id(latest_tree.get_last_sequence_id())
                .await?;
            latest_tree.apply_updates(&tree_updates);

            update_batching_tree()?;

            tree_updates
        }
        Ordering::Less => {
            debug!("Rewinding latest tree updates up to {}", new_sequence_id);
            update_batching_tree()?;

            latest_tree.rewind_updates_up_to(latest_tree_update.post_root);

            vec![]
        }
        Ordering::Equal => {
            debug!("Latest tree already up to date {}", new_sequence_id);

            update_batching_tree()?;

            vec![]
        }
    };

    Ok(tree_updates)
}

/// DB-free version of `update_latest_tree`, used in [`SyncPlan::apply`].
/// The incremental updates were already fetched during `build_sync_plan`.
fn apply_latest_tree<F: Fn() -> anyhow::Result<()>>(
    latest_tree: &TreeVersion<Latest>,
    latest_tree_update: &Option<TreeUpdate>,
    prefetched_updates: Vec<TreeUpdate>,
    update_batching_tree: F,
) -> anyhow::Result<Vec<TreeUpdate>> {
    let Some(latest_tree_update) = latest_tree_update else {
        debug!("No latest tree update.");
        update_batching_tree()?;
        return Ok(vec![]);
    };

    let current_sequence_id = latest_tree.get_last_sequence_id();
    let new_sequence_id = latest_tree_update.sequence_id;

    let tree_updates = match new_sequence_id.cmp(&current_sequence_id) {
        Ordering::Greater => {
            debug!("Applying latest tree updates up to {}", new_sequence_id);
            latest_tree.apply_updates(&prefetched_updates);
            update_batching_tree()?;
            prefetched_updates
        }
        Ordering::Less => {
            debug!("Rewinding latest tree updates up to {}", new_sequence_id);
            update_batching_tree()?;
            latest_tree.rewind_updates_up_to(latest_tree_update.post_root);
            vec![]
        }
        Ordering::Equal => {
            debug!("Latest tree already up to date {}", new_sequence_id);
            update_batching_tree()?;
            vec![]
        }
    };

    Ok(tree_updates)
}

fn update_batching_tree<F: Fn() -> anyhow::Result<()>>(
    batching_tree: &TreeVersion<Intermediate>,
    batching_tree_update: &Option<TreeUpdate>,
    update_processed_tree: F,
) -> anyhow::Result<()> {
    let Some(batching_tree_update) = batching_tree_update else {
        debug!("No batching tree update.");
        update_processed_tree()?;

        return Ok(());
    };

    let current_sequence_id = batching_tree.get_last_sequence_id();
    let new_sequence_id = batching_tree_update.sequence_id;

    match new_sequence_id.cmp(&current_sequence_id) {
        Ordering::Greater => {
            debug!("Applying batching tree updates up to {}", new_sequence_id);
            batching_tree.apply_updates_up_to(batching_tree_update.post_root);

            update_processed_tree()?;
        }
        Ordering::Less => {
            debug!("Rewinding batching tree updates up to {}", new_sequence_id);
            update_processed_tree()?;

            batching_tree.rewind_updates_up_to(batching_tree_update.post_root);
        }
        Ordering::Equal => {
            debug!("Batching tree already up to date {}", new_sequence_id);

            update_processed_tree()?;
        }
    }

    Ok(())
}

// apply_batching_tree is identical to update_batching_tree (no DB I/O needed).
fn apply_batching_tree<F: Fn() -> anyhow::Result<()>>(
    batching_tree: &TreeVersion<Intermediate>,
    batching_tree_update: &Option<TreeUpdate>,
    update_processed_tree: F,
) -> anyhow::Result<()> {
    update_batching_tree(batching_tree, batching_tree_update, update_processed_tree)
}

fn update_processed_tree<F: Fn() -> anyhow::Result<()>>(
    processed_tree: &TreeVersion<Intermediate>,
    processed_tree_update: &Option<TreeUpdate>,
    update_mined_tree: F,
) -> anyhow::Result<()> {
    let Some(processed_tree_update) = processed_tree_update else {
        debug!("No processed tree update.");
        update_mined_tree()?;

        return Ok(());
    };

    let current_sequence_id = processed_tree.get_last_sequence_id();
    let new_sequence_id = processed_tree_update.sequence_id;

    match new_sequence_id.cmp(&current_sequence_id) {
        Ordering::Greater => {
            debug!("Applying processed tree updates up to {}", new_sequence_id);
            processed_tree.apply_updates_up_to(processed_tree_update.post_root);

            update_mined_tree()?;
        }
        Ordering::Less => {
            debug!("Rewinding processed tree updates up to {}", new_sequence_id);
            update_mined_tree()?;

            processed_tree.rewind_updates_up_to(processed_tree_update.post_root);
        }
        Ordering::Equal => {
            debug!("Processed tree already up to date {}", new_sequence_id);

            update_mined_tree()?;
        }
    }

    Ok(())
}

// apply_processed_tree is identical to update_processed_tree.
fn apply_processed_tree<F: Fn() -> anyhow::Result<()>>(
    processed_tree: &TreeVersion<Intermediate>,
    processed_tree_update: &Option<TreeUpdate>,
    update_mined_tree: F,
) -> anyhow::Result<()> {
    update_processed_tree(processed_tree, processed_tree_update, update_mined_tree)
}

fn update_mined_tree(
    mined_tree: &TreeVersion<Canonical>,
    mined_tree_update: &Option<TreeUpdate>,
) -> anyhow::Result<()> {
    let Some(mined_tree_update) = mined_tree_update else {
        debug!("No mined tree update.");
        return Ok(());
    };

    let current_sequence_id = mined_tree.get_last_sequence_id();
    let new_sequence_id = mined_tree_update.sequence_id;

    match new_sequence_id.cmp(&current_sequence_id) {
        Ordering::Greater => {
            debug!("Applying mined tree updates up to {}", new_sequence_id);
            mined_tree.apply_updates_up_to(mined_tree_update.post_root);
        }
        Ordering::Less => {
            bail!("This should never happened. It is checked by assert done before calling.");
        }
        Ordering::Equal => {
            debug!("Mined tree already up to date {}", new_sequence_id);
        }
    }

    Ok(())
}

// apply_mined_tree is identical to update_mined_tree.
fn apply_mined_tree(
    mined_tree: &TreeVersion<Canonical>,
    mined_tree_update: &Option<TreeUpdate>,
) -> anyhow::Result<()> {
    update_mined_tree(mined_tree, mined_tree_update)
}
