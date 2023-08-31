use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result as AnyhowResult;
use ethers::types::U256;
use once_cell::sync::Lazy;
use prometheus::{register_histogram, Histogram};
use semaphore::poseidon_tree::Branch;
use tokio::sync::Notify;
use tokio::{select, time};
use tracing::{debug, error, info, instrument, warn};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{
    AppliedTreeUpdate, Intermediate, TreeVersion, TreeVersionReadOps, TreeWithNextVersion,
};
use crate::prover::identity::Identity;
use crate::prover::map::ReadOnlyInsertionProver;
use crate::task_monitor::{PendingBatchSubmission, TaskMonitor};
use crate::utils::async_queue::AsyncQueue;

/// The number of seconds either side of the timer tick to treat as enough to
/// trigger a forced batch insertion.
const DEBOUNCE_THRESHOLD_SECS: u64 = 1;

static PENDING_IDENTITIES_CHANNEL_CAPACITY: Lazy<Histogram> = Lazy::new(|| {
    register_histogram!(
        "pending_identities_channel_capacity",
        "Pending identities channel capacity"
    )
    .unwrap()
});

pub struct ProcessIdentities {
    database: Arc<Database>,
    identity_manager: SharedIdentityManager,
    batching_tree: TreeVersion<Intermediate>,
    batch_insert_timeout_secs: u64,
    pending_batch_submissions_queue: AsyncQueue<PendingBatchSubmission>,
    wake_up_notify: Arc<Notify>,
}

impl ProcessIdentities {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        batching_tree: TreeVersion<Intermediate>,
        batch_insert_timeout_secs: u64,
        pending_batch_submissions_queue: AsyncQueue<PendingBatchSubmission>,
        wake_up_notify: Arc<Notify>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            batching_tree,
            batch_insert_timeout_secs,
            pending_batch_submissions_queue,
            wake_up_notify,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        process_identities(
            &self.database,
            &self.identity_manager,
            &self.batching_tree,
            &self.wake_up_notify,
            &self.pending_batch_submissions_queue,
            self.batch_insert_timeout_secs,
        )
        .await
    }
}

async fn process_identities(
    database: &Database,
    identity_manager: &IdentityManager,
    batching_tree: &TreeVersion<Intermediate>,
    wake_up_notify: &Notify,
    pending_batch_submissions_queue: &AsyncQueue<PendingBatchSubmission>,
    timeout_secs: u64,
) -> AnyhowResult<()> {
    info!("Awaiting for a clean slate");
    identity_manager.await_clean_slate().await?;

    info!("Starting identity processor.");
    let batch_size = identity_manager.max_batch_size().await;

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
    let mut last_batch_time: SystemTime = SystemTime::now();

    loop {
        // We ping-pong between two cases for being woken. This ensures that there is a
        // maximum time that users can wait for their identity commitment to be
        // processed, but also that we are not inefficient with on-chain gas by being
        // too eager.
        select! {
            _ = timer.tick() => {
                debug!("Identity batch insertion woken due to timeout.");

                // If the timer has fired we want to insert whatever
                // identities we have, even if it's not many. This ensures
                // a minimum quality of service for API users.
                let updates = batching_tree.peek_next_updates(batch_size);
                if updates.is_empty() {
                    continue;
                }

                let prover = identity_manager.get_suitable_prover(updates.len()).await?;

                info!(
                    "Sending timed-out batch with {}/{} updates.",
                    updates.len(),
                    prover.batch_size()
                );

                commit_identities(
                    database,
                    identity_manager,
                    batching_tree,
                    pending_batch_submissions_queue,
                    &updates,
                    prover
                ).await?;

                last_batch_time = SystemTime::now();

                // Also wake up if woken up due to a tick
                wake_up_notify.notify_one();
            }
            _ = wake_up_notify.notified() => {
                tracing::trace!("Identity batch insertion woken due to request.");

                // Capture the time difference since the last batch, and compute
                // whether we want to insert anyway. We do this if the difference
                // is less than some debounce threshold.
                //
                // We unconditionally convert `u64 -> i64` as numbers should
                // always be small. If the numbers are not always small then
                // we _want_ to panic as something is horribly broken.
                let current_time = SystemTime::now();
                let diff_secs = if let Ok(diff) = current_time.duration_since(last_batch_time) {
                    diff.as_secs()
                } else {
                    warn!("Identity committer thinks that the last batch is in the future.");
                    continue
                };
                let should_process_anyway =
                    timeout_secs.abs_diff(diff_secs) <= DEBOUNCE_THRESHOLD_SECS;

                // We have _at most_ one complete batch here.
                let updates = batching_tree.peek_next_updates(batch_size);

                // If there are not enough identities to insert at this
                // stage we can wait. The timer will ensure that the API
                // clients do not wait too long for their submission to be
                // completed.
                if updates.len() < batch_size && !should_process_anyway {
                    // We do not reset the timer here as we may want to
                    // insert anyway soon.
                    tracing::trace!(
                        "Pending identities ({}) is less than batch size ({}). Waiting.",
                        updates.len(),
                        batch_size
                    );
                    continue;
                }

                let prover = identity_manager.get_suitable_prover(updates.len()).await?;

                commit_identities(
                    database,
                    identity_manager,
                    batching_tree,
                    pending_batch_submissions_queue,
                    &updates,
                    prover
                ).await?;

                // We've inserted the identities, so we want to ensure that
                // we don't trigger again until either we get a full batch
                // or the timer ticks.
                timer.reset();
                last_batch_time = SystemTime::now();

                // We want to check if there's a full batch available immediately
                wake_up_notify.notify_one();
            }
        }
    }
}

#[instrument(level = "info", skip_all)]
async fn commit_identities(
    database: &Database,
    identity_manager: &IdentityManager,
    batching_tree: &TreeVersion<Intermediate>,
    pending_batch_submissions_queue: &AsyncQueue<PendingBatchSubmission>,
    updates: &[AppliedTreeUpdate],
    insertion_prover: ReadOnlyInsertionProver<'_>,
) -> AnyhowResult<()> {
    TaskMonitor::log_identities_queues(database).await?;

    if updates.is_empty() {
        warn!("Identity commit requested with zero identities. Continuing.");
        return Ok(());
    }

    debug!("Starting identity commit for {} identities.", updates.len());

    // Sanity check that the insertions are to consecutive leaves in the tree.
    let mut last_index = updates
        .first()
        .expect("Updates is non empty.")
        .update
        .leaf_index;

    for update in updates[1..].iter() {
        assert_eq!(
            last_index + 1,
            update.update.leaf_index,
            "Identities are not consecutive leaves in the tree."
        );
        last_index = update.update.leaf_index;
    }

    // Grab the initial conditions before the updates are applied to the tree.
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

    let batch_size = insertion_prover.batch_size();

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
    let identity_commitments: Vec<Identity> = commitments
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
        .collect();

    identity_manager.validate_merkle_proofs(&identity_commitments)?;

    // We prepare the proof before reserving a slot in the pending identities
    let proof = IdentityManager::prepare_insertion_proof(
        insertion_prover,
        start_index,
        pre_root,
        post_root,
        &identity_commitments,
    )
    .await
    .map_err(|e| {
        error!(?e, "Failed to prepare proof.");
        e
    })?;

    #[allow(clippy::cast_precision_loss)]
    PENDING_IDENTITIES_CHANNEL_CAPACITY.observe(pending_batch_submissions_queue.len().await as f64);

    // This queue's capacity provides us with a natural back-pressure mechanism
    // to ensure that we don't overwhelm the identity manager with too many
    // identities to mine.
    let permit = pending_batch_submissions_queue.reserve().await;

    info!(start_index, ?pre_root, ?post_root, "Submitting batch");

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
            error!(?e, "Failed to insert identity to contract.");
            e
        })?;

    info!(
        start_index,
        ?pre_root,
        ?post_root,
        ?transaction_id,
        "Batch submitted"
    );

    // The transaction will be awaited on asynchronously
    permit
        .send(PendingBatchSubmission {
            transaction_id,
            pre_root,
            post_root,
            start_index,
        })
        .await;

    // Update the batching tree only after submitting the identities to the chain
    batching_tree.apply_updates_up_to(post_root.into());

    info!(start_index, ?pre_root, ?post_root, "Tree updated");

    TaskMonitor::log_batch_size(updates.len());

    Ok(())
}
