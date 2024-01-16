use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use ethers::types::U256;
use ruint::Uint;
use semaphore::merkle_tree::Proof;
use semaphore::poseidon_tree::{Branch, PoseidonHash};
use tokio::sync::{mpsc, Notify};
use tokio::{select, time};
use tracing::instrument;

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::ethereum::write::TransactionId;
use crate::identity_tree::{
    AppliedTreeUpdate, Hash, Intermediate, TreeVersion, TreeVersionReadOps, TreeWithNextVersion,
};
use crate::prover::identity::Identity;
use crate::prover::Prover;
use crate::task_monitor::TaskMonitor;
use crate::utils::batch_type::BatchType;
use crate::utils::index_packing::pack_indices;

/// The number of seconds either side of the timer tick to treat as enough to
/// trigger a forced batch insertion.
const DEBOUNCE_THRESHOLD_SECS: i64 = 1;

pub struct ProcessIdentities {
    database:                  Arc<Database>,
    identity_manager:          SharedIdentityManager,
    batching_tree:             TreeVersion<Intermediate>,
    batch_insert_timeout_secs: u64,
    monitored_txs_sender:      mpsc::Sender<TransactionId>,
    wake_up_notify:            Arc<Notify>,
}

impl ProcessIdentities {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        batching_tree: TreeVersion<Intermediate>,
        batch_insert_timeout_secs: u64,
        monitored_txs_sender: mpsc::Sender<TransactionId>,
        wake_up_notify: Arc<Notify>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            batching_tree,
            batch_insert_timeout_secs,
            monitored_txs_sender,
            wake_up_notify,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        process_identities(
            &self.database,
            &self.identity_manager,
            &self.batching_tree,
            &self.monitored_txs_sender,
            &self.wake_up_notify,
            self.batch_insert_timeout_secs,
        )
        .await
    }
}

async fn process_identities(
    database: &Database,
    identity_manager: &IdentityManager,
    batching_tree: &TreeVersion<Intermediate>,
    monitored_txs_sender: &mpsc::Sender<TransactionId>,
    wake_up_notify: &Notify,
    timeout_secs: u64,
) -> anyhow::Result<()> {
    tracing::info!("Awaiting for a clean slate");
    identity_manager.await_clean_slate().await?;

    tracing::info!("Starting identity processor.");

    // We start a timer and force it to perform one initial tick to avoid an
    // immediate trigger.
    let mut timer = time::interval(Duration::from_secs(timeout_secs));
    timer.tick().await;

    // When both futures are woken at once, the choice is made
    // non-deterministically. This could, in the worst case, result in users waiting
    // for twice `timeout_secs` for their insertion to be processed.
    //
    // To ensure that this does not happen we track the last time a batch was
    // inserted. If we have an incomplete batch but are within a small delta of the
    // tick happening anyway in the wake branch, we insert the current
    // (possibly-incomplete) batch anyway.
    let mut last_batch_time: DateTime<Utc> = database
        .get_latest_insertion_timestamp()
        .await?
        .unwrap_or(Utc::now());

    loop {
        // We wait either for a timer tick or a full batch
        select! {
            _ = timer.tick() => {
                tracing::info!("Identity batch insertion woken due to timeout");
            }
            () = wake_up_notify.notified() => {
                tracing::trace!("Identity batch insertion woken due to request");
            },
        }

        let Some(batch_type) = determine_batch_type(batching_tree) else {
            continue;
        };

        let batch_size = if batch_type.is_deletion() {
            identity_manager.max_deletion_batch_size().await
        } else {
            identity_manager.max_insertion_batch_size().await
        };

        let updates = batching_tree.peek_next_updates(batch_size);

        let current_time = Utc::now();
        let timeout_batch_time = last_batch_time
            + chrono::Duration::seconds(timeout_secs as i64)
            + chrono::Duration::seconds(DEBOUNCE_THRESHOLD_SECS);

        let can_skip_batch = current_time < timeout_batch_time;

        if updates.len() < batch_size && can_skip_batch {
            tracing::trace!(
                num_updates = updates.len(),
                batch_size,
                ?last_batch_time,
                "Pending identities is less than batch size, skipping batch",
            );

            continue;
        }

        commit_identities(
            database,
            identity_manager,
            batching_tree,
            monitored_txs_sender,
            &updates,
        )
        .await?;

        timer.reset();
        last_batch_time = Utc::now();
        database
            .update_latest_insertion_timestamp(last_batch_time)
            .await?;

        // We want to check if there's a full batch available immediately
        wake_up_notify.notify_one();
    }
}

async fn commit_identities(
    database: &Database,
    identity_manager: &IdentityManager,
    batching_tree: &TreeVersion<Intermediate>,
    monitored_txs_sender: &mpsc::Sender<TransactionId>,
    updates: &[AppliedTreeUpdate],
) -> anyhow::Result<()> {
    // If the update is an insertion
    let tx_id = if updates
        .first()
        .context("Updates should be > 1")?
        .update
        .element
        != Hash::ZERO
    {
        let prover = identity_manager
            .get_suitable_insertion_prover(updates.len())
            .await?;

        tracing::info!(
            num_updates = updates.len(),
            batch_size = prover.batch_size(),
            "Insertion batch",
        );

        insert_identities(database, identity_manager, batching_tree, updates, &prover).await?
    } else {
        let prover = identity_manager
            .get_suitable_deletion_prover(updates.len())
            .await?;

        tracing::info!(
            num_updates = updates.len(),
            batch_size = prover.batch_size(),
            "Deletion batch"
        );

        delete_identities(database, identity_manager, batching_tree, updates, &prover).await?
    };

    if let Some(tx_id) = tx_id {
        monitored_txs_sender.send(tx_id).await?;
    }

    Ok(())
}

#[instrument(level = "info", skip_all)]
pub async fn insert_identities(
    database: &Database,
    identity_manager: &IdentityManager,
    batching_tree: &TreeVersion<Intermediate>,
    updates: &[AppliedTreeUpdate],
    prover: &Prover,
) -> anyhow::Result<Option<TransactionId>> {
    assert_updates_are_consecutive(updates);

    let start_index = updates[0].update.leaf_index;
    let pre_root: U256 = batching_tree.get_root().into();
    let mut commitments: Vec<U256> = updates
        .iter()
        .map(|update| update.update.element.into())
        .collect();

    let latest_tree_from_updates = updates
        .last()
        .expect("Updates is non empty.")
        .result
        .clone();

    // Next get merkle proofs for each update - note the proofs are acquired from
    // intermediate versions of the tree
    let mut merkle_proofs: Vec<_> = updates
        .iter()
        .map(|update| update.result.proof(update.update.leaf_index))
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

    let batch_size = prover.batch_size();

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
    let post_root: U256 = latest_tree_from_updates.root().into();
    let identity_commitments = zip_commitments_and_proofs(commitments, merkle_proofs);

    identity_manager.validate_merkle_proofs(&identity_commitments)?;

    // We prepare the proof before reserving a slot in the pending identities
    let proof = IdentityManager::prepare_insertion_proof(
        prover,
        start_index,
        pre_root,
        &identity_commitments,
        post_root,
    )
    .await?;

    tracing::info!(
        start_index,
        ?pre_root,
        ?post_root,
        "Submitting insertion batch"
    );

    // With all the data prepared we can submit the identities to the on-chain
    // identity manager and wait for that transaction to be mined.
    let transaction_id = identity_manager
        .register_identities(
            start_index,
            pre_root,
            post_root,
            identity_commitments,
            proof,
        )
        .await
        .map_err(|e| {
            tracing::error!(?e, "Failed to insert identity to contract.");
            e
        })?;

    tracing::info!(
        start_index,
        ?pre_root,
        ?post_root,
        ?transaction_id,
        "Insertion batch submitted"
    );

    // Update the batching tree only after submitting the identities to the chain
    batching_tree.apply_updates_up_to(post_root.into());

    tracing::info!(start_index, ?pre_root, ?post_root, "Tree updated");

    TaskMonitor::log_batch_size(updates.len());

    Ok(Some(transaction_id))
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
    identity_manager: &IdentityManager,
    batching_tree: &TreeVersion<Intermediate>,
    updates: &[AppliedTreeUpdate],
    prover: &Prover,
) -> anyhow::Result<Option<TransactionId>> {
    // Grab the initial conditions before the updates are applied to the tree.
    let pre_root: U256 = batching_tree.get_root().into();

    let mut deletion_indices = updates
        .iter()
        .map(|f| f.update.leaf_index as u32)
        .collect::<Vec<u32>>();

    let commitments =
        batching_tree.commitments_by_indices(deletion_indices.iter().map(|x| *x as usize));
    let mut commitments: Vec<U256> = commitments.into_iter().map(U256::from).collect();

    let latest_tree_from_updates = updates
        .last()
        .expect("Updates is non empty.")
        .result
        .clone();

    // Next get merkle proofs for each update - note the proofs are acquired from
    // intermediate versions of the tree
    let mut merkle_proofs: Vec<_> = updates
        .iter()
        .map(|update_with_tree| {
            update_with_tree
                .result
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

    let batch_size = prover.batch_size();

    // The verifier and prover can only work with a given batch size, so we need to
    // ensure that our batches match that size. We do this by padding deletion
    // indices with tree.depth() ^ 2. The deletion prover will skip the proof for
    // any deletion with an index greater than the max tree depth
    let pad_index = 2_u32.pow(latest_tree_from_updates.depth() as u32);

    if commitment_count != batch_size {
        let padding = batch_size - commitment_count;
        commitments.extend(vec![U256::zero(); padding]);
        deletion_indices.extend(vec![pad_index; padding]);

        let zeroed_proof = Proof(vec![
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
    let post_root: U256 = latest_tree_from_updates.root().into();
    let identity_commitments = zip_commitments_and_proofs(commitments, merkle_proofs);

    identity_manager.validate_merkle_proofs(&identity_commitments)?;

    // We prepare the proof before reserving a slot in the pending identities
    let proof = IdentityManager::prepare_deletion_proof(
        prover,
        pre_root,
        deletion_indices.clone(),
        identity_commitments,
        post_root,
    )
    .await?;

    let packed_deletion_indices = pack_indices(&deletion_indices);

    tracing::info!(?pre_root, ?post_root, "Submitting deletion batch");

    // With all the data prepared we can submit the identities to the on-chain
    // identity manager and wait for that transaction to be mined.
    let transaction_id = identity_manager
        .delete_identities(proof, packed_deletion_indices, pre_root, post_root)
        .await
        .map_err(|e| {
            tracing::error!(?e, "Failed to insert identity to contract.");
            e
        })?;

    tracing::info!(
        ?pre_root,
        ?post_root,
        ?transaction_id,
        "Deletion batch submitted"
    );

    // Update the batching tree only after submitting the identities to the chain
    batching_tree.apply_updates_up_to(post_root.into());

    tracing::info!(?pre_root, ?post_root, "Tree updated");

    TaskMonitor::log_batch_size(updates.len());

    Ok(Some(transaction_id))
}

fn zip_commitments_and_proofs(
    commitments: Vec<U256>,
    merkle_proofs: Vec<Proof<PoseidonHash>>,
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

fn determine_batch_type(tree: &TreeVersion<Intermediate>) -> Option<BatchType> {
    let next_update = tree.peek_next_updates(1);
    if next_update.is_empty() {
        return None;
    }

    let batch_type = if next_update[0].update.element == Hash::ZERO {
        BatchType::Deletion
    } else {
        BatchType::Insertion
    };

    Some(batch_type)
}
