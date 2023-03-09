use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::{anyhow, Result as AnyhowResult};
use clap::Parser;
use ethers::types::U256;
use semaphore::merkle_tree::Branch;
use tokio::{
    select,
    sync::{
        mpsc::{self, error::TrySendError, Receiver},
        RwLock,
    },
    task::JoinHandle,
    time,
};
use tracing::{debug, error, info, instrument, warn};

use crate::{
    contracts::{IdentityManager, SharedIdentityManager},
    database::Database,
    identity_tree::{TreeState, TreeUpdate, TreeVersion},
    prover::batch_insertion::Identity,
    utils::spawn_or_abort,
};

/// The number of seconds either side of the timer tick to treat as enough to
/// trigger a forced batch insertion.
const DEBOUNCE_THRESHOLD_SECS: u64 = 1;

struct RunningInstance {
    handle:          JoinHandle<()>,
    wake_up_sender:  mpsc::Sender<()>,
    shutdown_sender: mpsc::Sender<()>,
}

impl RunningInstance {
    fn wake_up(&self) -> AnyhowResult<()> {
        // We're using a 1-element channel for wake-up notifications. It is safe to
        // ignore a full channel, because that means the committer is already scheduled
        // to wake up and will process all requests inserted in the database.
        match self.wake_up_sender.try_send(()) {
            Ok(_) => {
                debug!("Scheduled a committer job.");
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                debug!("Committer job already scheduled.");
                Ok(())
            }
            Err(TrySendError::Closed(_)) => {
                Err(anyhow!("Committer thread terminated unexpectedly."))
            }
        }
    }

    async fn shutdown(self) -> AnyhowResult<()> {
        info!("Sending a shutdown signal to the committer.");
        // Ignoring errors here, since we have two options: either the channel is full,
        // which is impossible, since this is the only use, and this method takes
        // ownership, or the channel is closed, which means the committer thread is
        // already dead.
        let _: Result<_, _> = self.shutdown_sender.send(()).await;
        info!("Awaiting committer shutdown.");
        self.handle.await?;
        Ok(())
    }
}

/// Configuration options for the component responsible for committing
/// identities when queried.
#[derive(Clone, Debug, PartialEq, Eq, Parser)]
#[group(skip)]
pub struct Options {
    /// The maximum number of seconds the sequencer will wait before sending a
    /// batch of identities to the chain, even if the batch is not full.
    #[clap(long, env, default_value = "180")]
    pub batch_timeout_seconds: u64,
}

/// A worker that commits identities to the blockchain.
///
/// This uses the database to keep track of identities that need to be
/// committed. It assumes that there's only one such worker spawned at
/// a time. Spawning multiple worker threads will result in undefined behavior,
/// including data duplication.
pub struct IdentityCommitter {
    instance:                  RwLock<Option<RunningInstance>>,
    database:                  Arc<Database>,
    identity_manager:          SharedIdentityManager,
    tree_state:                TreeState,
    batch_insert_timeout_secs: u64,
}

impl IdentityCommitter {
    pub fn new(
        database: Arc<Database>,
        contracts: SharedIdentityManager,
        tree_state: TreeState,
        options: &Options,
    ) -> Self {
        let batch_insert_timeout_secs = options.batch_timeout_seconds;
        Self {
            instance: RwLock::new(None),
            database,
            identity_manager: contracts,
            tree_state,
            batch_insert_timeout_secs,
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn start(&self) {
        let mut instance = self.instance.write().await;
        if instance.is_some() {
            warn!("Identity committer already running");
            return;
        }
        let (shutdown_sender, mut shutdown_receiver) = mpsc::channel(1);
        let (wake_up_sender, mut wake_up_receiver) = mpsc::channel(1);
        let database = self.database.clone();
        let identity_manager = self.identity_manager.clone();
        let batch_tree = self.tree_state.get_batching_tree();
        let mined_tree = self.tree_state.get_mined_tree();
        let timeout = self.batch_insert_timeout_secs;
        let handle = spawn_or_abort(async move {
            select! {
                result = Self::process_identities(
                    &database,
                    &identity_manager,
                    &batch_tree,
                    &mined_tree,
                    &mut wake_up_receiver,
                    timeout
                ) => {
                    result?;
                }
                _ = shutdown_receiver.recv() => {
                    info!("Woke up by shutdown signal, exiting.");
                    return Ok(());
                }
            }
            Ok(())
        });
        *instance = Some(RunningInstance {
            handle,
            wake_up_sender,
            shutdown_sender,
        });
    }

    async fn process_identities(
        database: &Database,
        identity_manager: &IdentityManager,
        batching_tree: &TreeVersion,
        mined_tree: &TreeVersion,
        wake_up_receiver: &mut Receiver<()>,
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
                        mined_tree,
                        batching_tree,
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
                        mined_tree,
                        batching_tree,
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

    // TODO This can be split into multiple phases. The first would compute the
    //   batches and the second would mine those batches.
    #[instrument(level = "info", skip_all)]
    async fn commit_identities(
        database: &Database,
        identity_manager: &IdentityManager,
        mined_tree: &TreeVersion,
        batching_tree: &TreeVersion,
        updates: &[TreeUpdate],
    ) -> AnyhowResult<()> {
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

        // With all the data prepared we can submit the identities to the on-chain
        // identity manager and wait for that transaction to be mined.
        identity_manager
            .register_identities(start_index, pre_root, post_root, identity_commitments)
            .await
            .map_err(|e| {
                error!(?e, "Failed to insert identity to contract.");
                e
            })?;

        // With this done, all that remains is to mark them as submitted to the
        // blockchain in the source-of-truth database, and also update the mined tree to
        // agree with the database and chain.
        let identity_keys: Vec<usize> = updates.iter().map(|update| update.leaf_index).collect();
        database
            .mark_identities_submitted_to_contract(identity_keys.as_slice())
            .await?;
        mined_tree.apply_next_updates(updates.len()).await;

        Ok(())
    }

    pub async fn notify_queued(&self) {
        // Escalate all errors to panics. In the future could perform some
        // restart procedure here.
        self.instance
            .read()
            .await
            .as_ref()
            .expect("Committer not running, terminating.")
            .wake_up()
            .unwrap();
    }

    /// # Errors
    ///
    /// Will return an Error if the committer thread cannot be shut down
    /// gracefully.
    pub async fn shutdown(&self) -> AnyhowResult<()> {
        let mut instance = self.instance.write().await;
        if let Some(instance) = instance.take() {
            instance.shutdown().await?;
        } else {
            info!("Committer not running.");
        }
        Ok(())
    }
}
