use chrono::{DateTime, Utc};
use ethers::prelude::U256;
use ruint::Uint;
use semaphore_rs::poseidon_tree::Branch;
use semaphore_rs_poseidon::Poseidon as PoseidonHash;
use semaphore_rs_trees::InclusionProof;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch::Receiver;
use tokio::sync::Notify;
use tokio::time::MissedTickBehavior;
use tokio::{select, time};
use tracing::instrument;

use crate::app::App;
use crate::database;
use crate::database::methods::DbMethods as _;
use crate::database::Database;
use crate::identity_tree::{
    AppliedTreeUpdate, Hash, Intermediate, TreeVersion, TreeVersionReadOps, TreeWithNextVersion,
};
use crate::monitoring::Monitoring;
use crate::prover::identity::Identity;
use crate::prover::repository::ProverRepository;
use crate::utils::batch_type::BatchType;

/// The number of seconds either side of the timer tick to treat as enough to
/// trigger a forced batch insertion.
const DEBOUNCE_THRESHOLD_SECS: i64 = 1;

pub async fn create_batches(
    app: Arc<App>,
    next_batch_notify: Arc<Notify>,
    sync_tree_notify: Arc<Notify>,
    mut tree_synced_rx: Receiver<()>,
) -> anyhow::Result<()> {
    tracing::info!("Starting batch creator.");
    ensure_batch_chain_initialized(&app).await?;

    // We start a timer and force it to perform one initial tick to avoid an
    // immediate trigger.
    let mut timer = time::interval(Duration::from_secs(5));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        // We wait either for a timer tick or a full batch
        select! {
            _ = timer.tick() => {
                tracing::info!("Create batches woken due to timeout");
            }

            res = tree_synced_rx.changed() => {
                if res.is_err() {
                    tracing::trace!("Tree synced channel closed. Quiting.");
                    return Ok(res?);
                }
                tracing::trace!("Create batches woken due tree synced event");
            },
        }

        let (batch_type, next_batch_type, max_batch_size, updates, pre_root, pre_commitments) = {
            let tree_state = app.tree_state().await?;

            let Some(batch_type) = determine_next_batch_type(tree_state.batching_tree()) else {
                continue;
            };

            let max_batch_size = if batch_type.is_deletion() {
                app.prover_repository.max_deletion_batch_size().await
            } else {
                app.prover_repository.max_insertion_batch_size().await
            };

            let pre_root = tree_state.batching_tree().get_root();

            let updates = tree_state.batching_tree().peek_next_updates(max_batch_size);

            let updates_indices: Vec<_> = updates.iter().map(|f| f.update.leaf_index).collect();

            let pre_commitments = tree_state
                .batching_tree()
                .commitments_by_leaves(updates_indices.iter().copied());

            let next_batch_type = tree_state
                .batching_tree()
                .peek_next_update_at(updates.len())
                .map(|update| determine_batch_type(&update));

            (
                batch_type,
                next_batch_type,
                max_batch_size,
                updates,
                pre_root,
                pre_commitments,
            )
        };

        if updates.is_empty() {
            tracing::trace!("No updates found. Waiting.");
            continue;
        }

        // If the batch is a deletion, process immediately without resetting the timer
        if batch_type.is_deletion() {
            commit_deletion_batch(
                &app.database,
                &app.prover_repository,
                &next_batch_notify,
                &sync_tree_notify,
                pre_root,
                &updates,
                &pre_commitments,
            )
            .await?;

            continue;
        }

        let current_time = Utc::now();
        let batch_insertion_timeout =
            chrono::Duration::from_std(app.config.app.batch_insertion_timeout)?;

        let last_batch_time: DateTime<Utc> = app.database.get_latest_insertion().await?.timestamp;

        let timeout_batch_time = last_batch_time
            + batch_insertion_timeout
            + chrono::Duration::seconds(DEBOUNCE_THRESHOLD_SECS);

        let batch_time_elapsed = current_time >= timeout_batch_time;

        // If the batch size is full or if the insertion time has elapsed
        // process the batch
        if updates.len() >= max_batch_size || batch_time_elapsed {
            commit_insertion_batch(
                &app.database,
                &app.prover_repository,
                &next_batch_notify,
                &sync_tree_notify,
                pre_root,
                &updates,
            )
            .await?;

            // We've inserted the identities, so we want to ensure that
            // we don't trigger again until either we get a full batch
            // or the timer ticks.
            app.database.update_latest_insertion(Utc::now()).await?;

            continue;
        }

        // Check if the next batch after the current insertion batch is
        // deletion. The only time that deletions are
        // inserted is when there is a full deletion batch or the
        // deletion time interval has elapsed.
        // In this case, we should immediately process the batch.
        let next_update_is_deletion = if let Some(next_batch_type) = next_batch_type {
            next_batch_type == BatchType::Deletion
        } else {
            false
        };

        // If the next batch is deletion, process the current insertion batch
        if next_update_is_deletion {
            commit_insertion_batch(
                &app.database,
                &app.prover_repository,
                &next_batch_notify,
                &sync_tree_notify,
                pre_root,
                &updates,
            )
            .await?;

            continue;
        }

        // If there are not enough identities to fill the batch, the time interval has
        // not elapsed and the next batch is not deletion, wait for more identities
        tracing::trace!(
            "Pending identities ({}) is less than max batch size ({}). Waiting.",
            updates.len(),
            max_batch_size
        );
    }
}

async fn ensure_batch_chain_initialized(app: &Arc<App>) -> anyhow::Result<()> {
    let batch_head = app.database.get_batch_head().await?;
    if batch_head.is_none() {
        app.database
            .insert_new_batch_head(&app.tree_state().await?.batching_tree().get_root())
            .await?;
    }
    Ok(())
}

async fn commit_insertion_batch(
    database: &Database,
    prover_repository: &Arc<ProverRepository>,
    next_batch_notify: &Arc<Notify>,
    sync_tree_notify: &Arc<Notify>,
    pre_root: Hash,
    updates: &[AppliedTreeUpdate],
) -> anyhow::Result<()> {
    let batch_size = prover_repository
        .get_suitable_insertion_batch_size(updates.len())
        .await?;

    tracing::info!(num_updates = updates.len(), batch_size, "Insertion batch",);

    insert_identities(
        database,
        next_batch_notify,
        sync_tree_notify,
        pre_root,
        updates,
        batch_size,
    )
    .await
}

async fn commit_deletion_batch(
    database: &Database,
    prover_repository: &Arc<ProverRepository>,
    next_batch_notify: &Arc<Notify>,
    sync_tree_notify: &Arc<Notify>,
    pre_root: Hash,
    updates: &[AppliedTreeUpdate],
    pre_commitments: &[Hash],
) -> anyhow::Result<()> {
    let batch_size = prover_repository
        .get_suitable_deletion_batch_size(updates.len())
        .await?;

    tracing::info!(num_updates = updates.len(), batch_size, "Deletion batch");

    delete_identities(
        database,
        next_batch_notify,
        sync_tree_notify,
        pre_root,
        updates,
        pre_commitments,
        batch_size,
    )
    .await
}

#[instrument(level = "info", skip_all)]
pub async fn insert_identities(
    database: &Database,
    next_batch_notify: &Arc<Notify>,
    sync_tree_notify: &Arc<Notify>,
    pre_root: Hash,
    updates: &[AppliedTreeUpdate],
    batch_size: usize,
) -> anyhow::Result<()> {
    assert_updates_are_consecutive(updates);

    let mut tx = database.pool.begin().await?;
    let latest_batch = tx.get_latest_batch().await?;
    if let Some(latest_batch) = latest_batch {
        if pre_root != latest_batch.next_root {
            // Tree not synced
            sync_tree_notify.notify_one();
            return Ok(());
        }
    }

    let mut insertion_indices: Vec<_> = updates.iter().map(|f| f.update.leaf_index).collect();
    let mut commitments: Vec<U256> = updates
        .iter()
        .map(|update| update.update.element.into())
        .collect();

    let latest_tree_from_updates = updates
        .last()
        .expect("Updates is non empty.")
        .post_state
        .tree
        .clone();

    // Next get merkle proofs for each update - note the proofs are acquired from
    // intermediate versions of the tree
    let mut merkle_proofs: Vec<_> = updates
        .iter()
        .map(|update| update.post_state.tree.proof(update.update.leaf_index))
        .collect();

    // Grab some variables for sizes to make querying easier.
    let commitment_count = updates.len();

    // If these aren't equal then something has gone terribly wrong and is a
    // programmer bug, so we abort.
    assert_eq!(
        commitment_count,
        merkle_proofs.len(),
        "Number of identities does not match the number of merkle proofs."
    );

    // The verifier and prover can only work with a given batch size, so we need to
    // ensure that our batches match that size. We do this by padding with
    // subsequent zero identities and their associated merkle proofs if the batch is
    // too small.
    if commitment_count != batch_size {
        let start_index = updates
            .last()
            .expect("Already confirmed to exist.")
            .update
            .leaf_index
            + 1;
        let padding = batch_size - commitment_count;
        commitments.append(&mut vec![U256::zero(); padding]);

        for i in start_index..(start_index + padding) {
            let proof = latest_tree_from_updates.proof(i);
            merkle_proofs.push(proof);
            insertion_indices.push(i);
        }
    }

    assert_eq!(
        commitments.len(),
        batch_size,
        "Mismatch between commitments and batch size."
    );
    assert_eq!(
        merkle_proofs.len(),
        batch_size,
        "Mismatch between merkle proofs and batch size."
    );

    // With the updates applied we can grab the value of the tree's new root and
    // build our identities for sending to the identity manager.
    let post_root = latest_tree_from_updates.root();
    let identity_commitments = zip_commitments_and_proofs(commitments, merkle_proofs);
    let start_index = *insertion_indices.first().unwrap();

    tracing::info!(
        start_index,
        ?pre_root,
        ?post_root,
        "Submitting insertion batch to DB"
    );

    // With all the data prepared we can submit the batch to database.
    tx.insert_new_batch(
        &post_root,
        &pre_root,
        database::types::BatchType::Insertion,
        &identity_commitments,
        &insertion_indices,
    )
    .await?;

    // It is important to commit transaction as soon as possible.
    tx.commit().await?;

    tracing::info!(
        start_index,
        ?pre_root,
        ?post_root,
        "Insertion batch submitted to DB"
    );

    next_batch_notify.notify_one();

    Monitoring::log_batch_size(updates.len());

    sync_tree_notify.notify_one();

    Ok(())
}

fn assert_updates_are_consecutive(updates: &[AppliedTreeUpdate]) {
    for updates in updates.windows(2) {
        let first = &updates[0];
        let second = &updates[1];

        if first.update.leaf_index + 1 != second.update.leaf_index {
            let leaf_indexes = updates
                .iter()
                .map(|update| update.update.leaf_index)
                .collect::<Vec<_>>();
            let commitments = updates
                .iter()
                .map(|update| update.update.element)
                .collect::<Vec<_>>();

            panic!(
                "Identities are not consecutive leaves in the tree (leaf_indexes = {:?}, \
                 commitments = {:?})",
                leaf_indexes, commitments
            );
        }
    }
}

pub async fn delete_identities(
    database: &Database,
    next_batch_notify: &Arc<Notify>,
    sync_tree_notify: &Arc<Notify>,
    pre_root: Hash,
    updates: &[AppliedTreeUpdate],
    pre_commitments: &[Hash],
    batch_size: usize,
) -> anyhow::Result<()> {
    let mut tx = database.pool.begin().await?;
    let latest_batch = tx.get_latest_batch().await?;
    if let Some(latest_batch) = latest_batch {
        if pre_root != latest_batch.next_root {
            // Tree not synced
            sync_tree_notify.notify_one();
            return Ok(());
        }
    }

    let mut deletion_indices: Vec<_> = updates.iter().map(|f| f.update.leaf_index).collect();
    let mut commitments: Vec<U256> = pre_commitments.iter().map(|c| U256::from(*c)).collect();

    let latest_tree_from_updates = updates
        .last()
        .expect("Updates is non empty.")
        .post_state
        .tree
        .clone();

    // Next get merkle proofs for each update - note the proofs are acquired from
    // intermediate versions of the tree
    let mut merkle_proofs: Vec<_> = updates
        .iter()
        .map(|update_with_tree| {
            update_with_tree
                .post_state
                .tree
                .proof(update_with_tree.update.leaf_index)
        })
        .collect();

    // Grab some variables for sizes to make querying easier.
    let commitment_count = updates.len();

    // If these aren't equal then something has gone terribly wrong and is a
    // programmer bug, so we abort.
    assert_eq!(
        commitment_count,
        merkle_proofs.len(),
        "Number of identities does not match the number of merkle proofs."
    );

    // The verifier and prover can only work with a given batch size, so we need to
    // ensure that our batches match that size. We do this by padding deletion
    // indices with tree.depth() ^ 2. The deletion prover will skip the proof for
    // any deletion with an index greater than the max tree depth
    let pad_index = 2_u32.pow(latest_tree_from_updates.depth() as u32) as usize;

    if commitment_count != batch_size {
        let padding = batch_size - commitment_count;
        commitments.extend(vec![U256::zero(); padding]);
        deletion_indices.extend(vec![pad_index; padding]);

        let zeroed_proof = InclusionProof(vec![
            Branch::Left(Uint::ZERO);
            latest_tree_from_updates.depth()
        ]);

        merkle_proofs.extend(vec![zeroed_proof; padding]);
    }

    assert_eq!(
        deletion_indices.len(),
        batch_size,
        "Mismatch between deletion indices length and batch size."
    );

    // With the updates applied we can grab the value of the tree's new root and
    // build our identities for sending to the identity manager.
    let post_root = latest_tree_from_updates.root();
    let identity_commitments = zip_commitments_and_proofs(commitments, merkle_proofs);

    tracing::info!(?pre_root, ?post_root, "Submitting deletion batch to DB");

    // With all the data prepared we can submit the batch to database.
    tx.insert_new_batch(
        &post_root,
        &pre_root,
        database::types::BatchType::Deletion,
        &identity_commitments,
        &deletion_indices,
    )
    .await?;

    // It is important to commit transaction as soon as possible.
    tx.commit().await?;

    tracing::info!(?pre_root, ?post_root, "Deletion batch submitted to DB");

    next_batch_notify.notify_one();

    Monitoring::log_batch_size(updates.len());

    sync_tree_notify.notify_one();

    Ok(())
}

fn determine_next_batch_type(tree: &TreeVersion<Intermediate>) -> Option<BatchType> {
    let next_update = tree.peek_next_updates(1);
    if next_update.is_empty() {
        return None;
    }

    let batch_type = determine_batch_type(&next_update[0]);

    Some(batch_type)
}

fn determine_batch_type(update: &AppliedTreeUpdate) -> BatchType {
    if update.update.element == Hash::ZERO {
        BatchType::Deletion
    } else {
        BatchType::Insertion
    }
}

fn zip_commitments_and_proofs(
    commitments: Vec<U256>,
    merkle_proofs: Vec<InclusionProof<PoseidonHash>>,
) -> Vec<Identity> {
    commitments
        .iter()
        .zip(merkle_proofs)
        .map(|(id, prf)| {
            let commitment: U256 = id.into();
            let proof: Vec<U256> = prf
                .0
                .iter()
                .map(|branch| match branch {
                    Branch::Left(v) | Branch::Right(v) => U256::from(*v),
                })
                .collect();
            Identity::new(commitment, proof)
        })
        .collect()
}
