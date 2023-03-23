use std::time::{Duration, SystemTime};

use anyhow::Result as AnyhowResult;
use ethers::types::U256;
use semaphore::poseidon_tree::Branch;
use tokio::{select, sync::mpsc, time};
use tracing::{debug, error, info, instrument, warn};

use crate::{
    contracts::IdentityManager,
    database::Database,
    identity_committer::PendingIdentities,
    identity_tree::{TreeUpdate, TreeVersion},
    prover::batch_insertion::Identity,
};

use crate::identity_committer::IdentityCommitter;

/// The number of seconds either side of the timer tick to treat as enough to
/// trigger a forced batch insertion.
const DEBOUNCE_THRESHOLD_SECS: u64 = 1;

impl IdentityCommitter {
    pub async fn process_identities(
        database: &Database,
        identity_manager: &IdentityManager,
        batching_tree: &TreeVersion,
        wake_up_receiver: &mut mpsc::Receiver<()>,
        pending_identities_sender: &mpsc::Sender<PendingIdentities>,
        timeout_secs: u64,
    ) -> AnyhowResult<()> {
        info!("Starting identity processor.");
        let batch_size = identity_manager.batch_size();

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
            //
            select! {
                _ = timer.tick() => {
                    debug!("Identity batch insertion woken due to timeout.");

                    // If the timer has fired we want to insert whatever
                    // identities we have, even if it's not many. This ensures
                    // a minimum quality of service for API users.
                    let updates = batching_tree.peek_next_updates(batch_size).await;
                    if updates.is_empty() {
                        continue;
                    }
                    info!("Sending non-full batch with {}/{} updates.", updates.len(), batch_size);

                    Self::commit_identities(
                        database,
                        identity_manager,
                        batching_tree,
                        pending_identities_sender,
                        &updates
                    ).await?;

                    last_batch_time = SystemTime::now();
                }
                _ = wake_up_receiver.recv() => {
                    debug!("Identity batch insertion woken due to request.");

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
                        warn!("Identity committer things that the last batch is in the future.");
                        continue
                    };
                    let should_process_anyway =
                        timeout_secs.abs_diff(diff_secs) <= DEBOUNCE_THRESHOLD_SECS;

                    // We have _at most_ one complete batch here.
                    let updates = batching_tree.peek_next_updates(batch_size).await;

                    // If there are not enough identities to insert at this
                    // stage we can wait. The timer will ensure that the API
                    // clients do not wait too long for their submission to be
                    // completed.
                    if updates.len() < batch_size && !should_process_anyway {
                        // We do not reset the timer here as we may want to
                        // insert anyway soon.
                        debug!(
                            "Pending identities ({}) is less than batch size ({}). Waiting.",
                            updates.len(),
                            batch_size
                        );
                        continue;
                    }

                    Self::commit_identities(
                        database,
                        identity_manager,
                        batching_tree,
                        pending_identities_sender,
                        &updates
                    ).await?;

                    // We've inserted the identities, so we want to ensure that
                    // we don't trigger again until either we get a full batch
                    // or the timer ticks.
                    timer.reset();
                    last_batch_time = SystemTime::now();
                }
            }
        }
    }

    #[instrument(level = "info", skip_all)]
    async fn commit_identities(
        database: &Database,
        identity_manager: &IdentityManager,
        batching_tree: &TreeVersion,
        pending_identities_sender: &mpsc::Sender<PendingIdentities>,
        updates: &[TreeUpdate],
    ) -> AnyhowResult<()> {
        Self::log_pending_identities_count(database).await?;

        if updates.is_empty() {
            warn!("Identity commit requested with zero identities. Continuing.");
            return Ok(());
        }
        debug!("Starting identity commit for {} identities.", updates.len());

        // Sanity check that the insertions are to consecutive leaves in the tree.
        let mut last_index = updates.first().expect("Updates is non empty.").leaf_index;
        for update in updates[1..].iter() {
            assert_eq!(
                last_index + 1,
                update.leaf_index,
                "Identities are not consecutive leaves in the tree."
            );
            last_index = update.leaf_index;
        }

        // Grab the initial conditions before the updates are applied to the tree.
        let start_index = updates[0].leaf_index;
        let pre_root: U256 = batching_tree.get_root().await.into();
        let mut commitments: Vec<U256> =
            updates.iter().map(|update| update.element.into()).collect();

        // Next we apply the updates, retrieving the merkle proofs after each step of
        // that process.
        let mut merkle_proofs = batching_tree.apply_next_updates(updates.len()).await;

        // Grab some variables for sizes to make querying easier.
        let batch_size = identity_manager.batch_size();
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
                .leaf_index
                + 1;
            let padding = batch_size - commitment_count;
            commitments.append(&mut vec![U256::zero(); padding]);

            for i in start_index..(start_index + padding) {
                let (_, proof) = batching_tree.get_proof(i).await;
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
        let post_root: U256 = batching_tree.get_root().await.into();
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

        // We prepare the proof before reserving a slot in the pending identities
        let proof = identity_manager
            .prepare_proof(start_index, pre_root, post_root, &identity_commitments)
            .await
            .map_err(|e| {
                error!(?e, "Failed to prepare proof.");
                e
            })?;

        // This channel's capacity provides us with a natural back-pressure mechanism
        // to ensure that we don't overwhelm the identity manager with too many
        // identities to mine.
        //
        // Additionally if the receiver is dropped this reserve call will also fail.
        let permit = pending_identities_sender.reserve().await?;

        // Ensure that we are not going to submit based on an out of date root anyway.
        identity_manager.assert_latest_root(pre_root.into()).await?;

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

        let identity_keys: Vec<usize> = updates.iter().map(|update| update.leaf_index).collect();

        // The transaction will be awaited on asynchronously
        permit.send(PendingIdentities {
            identity_keys,
            transaction_id,
            pre_root,
            post_root,
            start_index,
        });

        Self::log_batch_size(updates.len()).await?;

        Ok(())
    }
}
