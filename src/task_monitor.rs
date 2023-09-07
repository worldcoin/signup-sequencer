use std::sync::Arc;
use std::time::Duration;

use anyhow::Result as AnyhowResult;
use clap::Parser;
use ethers::types::U256;
use once_cell::sync::Lazy;
use prometheus::{linear_buckets, register_gauge, register_histogram, Gauge, Histogram};
use tokio::sync::{broadcast, Notify, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, instrument, warn};

use self::tasks::finalize_identities::FinalizeRoots;
use self::tasks::insert_identities::InsertIdentities;
use self::tasks::mine_identities::MineIdentities;
use self::tasks::process_identities::ProcessIdentities;
use crate::contracts::SharedIdentityManager;
use crate::database::Database;
use crate::ethereum::write::TransactionId;
use crate::identity_tree::TreeState;
use crate::utils::async_queue::AsyncQueue;

pub mod tasks;

const PROCESS_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const FINALIZE_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const MINE_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const INSERT_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);

struct RunningInstance {
    handles:         Vec<JoinHandle<()>>,
    shutdown_sender: broadcast::Sender<()>,
}

#[derive(Debug, Clone)]
pub struct PendingBatchSubmission {
    transaction_id: TransactionId,
    pre_root:       U256,
    post_root:      U256,
    start_index:    usize,
}

static PENDING_IDENTITIES: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!("pending_identities", "Identities not submitted on-chain").unwrap()
});

static UNPROCESSED_IDENTITIES: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!(
        "unprocessed_identities",
        "Identities not processed by identity committer"
    )
    .unwrap()
});

static BATCH_SIZES: Lazy<Histogram> = Lazy::new(|| {
    register_histogram!(
        "submitted_batch_sizes",
        "Submitted batch size",
        linear_buckets(f64::from(1), f64::from(1), 100).unwrap()
    )
    .unwrap()
});

impl RunningInstance {
    async fn shutdown(self) -> AnyhowResult<()> {
        info!("Sending a shutdown signal to the committer.");
        // Ignoring errors here, since we have two options: either the channel is full,
        // which is impossible, since this is the only use, and this method takes
        // ownership, or the channel is closed, which means the committer thread is
        // already dead.
        _ = self.shutdown_sender.send(());

        info!("Awaiting tasks to shutdown.");
        for result in futures::future::join_all(self.handles).await {
            result?;
        }

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

    /// How many transactions can be sent "at once" to the blockchain via the
    /// write provider.
    #[clap(long, env, default_value = "1")]
    pub pending_identities_capacity: usize,

    /// How many roots can be held in the mined roots queue at any given time.
    ///
    /// There is no reason why we shouldn't be able to wait for multiple
    /// roots to be finalized across chains at the same time.
    ///
    /// This is just a limit on memory usage for this channel.
    #[clap(long, env, default_value = "10")]
    pub mined_roots_capacity: usize,

    /// The maximum number of attempts to finalize a root before giving up.
    #[clap(long, env, default_value = "100")]
    pub finalization_max_attempts: usize,

    /// The number of seconds to wait between attempts to finalize a root.
    #[clap(long, env, default_value = "30")]
    pub finalization_sleep_time_seconds: u64,
}

/// A worker that commits identities to the blockchain.
///
/// This uses the database to keep track of identities that need to be
/// committed. It assumes that there's only one such worker spawned at
/// a time. Spawning multiple worker threads will result in undefined behavior,
/// including data duplication.
pub struct TaskMonitor {
    /// The instance is kept behind an RwLock<Option<...>> because
    /// when shutdown is called we want to be able to gracefully
    /// await the join handles - which requires ownership of the handle and by
    /// extension the instance.
    instance:                    RwLock<Option<RunningInstance>>,
    database:                    Arc<Database>,
    identity_manager:            SharedIdentityManager,
    tree_state:                  TreeState,
    batch_insert_timeout_secs:   u64,
    pending_identities_capacity: usize,
    mined_roots_capacity:        usize,

    finalization_max_attempts:       usize,
    finalization_sleep_time_seconds: u64,
}

impl TaskMonitor {
    pub fn new(
        database: Arc<Database>,
        contracts: SharedIdentityManager,
        tree_state: TreeState,
        options: &Options,
    ) -> Self {
        let Options {
            batch_timeout_seconds,
            pending_identities_capacity,
            mined_roots_capacity,
            finalization_max_attempts,
            finalization_sleep_time_seconds,
        } = *options;

        Self {
            instance: RwLock::new(None),
            database,
            identity_manager: contracts,
            tree_state,
            batch_insert_timeout_secs: batch_timeout_seconds,
            pending_identities_capacity,
            mined_roots_capacity,
            finalization_max_attempts,
            finalization_sleep_time_seconds,
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn start(&self) {
        let mut instance = self.instance.write().await;
        if instance.is_some() {
            warn!("Identity committer already running");
        }

        // We could use the second element of the tuple as `mut shutdown_receiver`,
        // but for symmetry's sake we create it for every task with `.subscribe()`
        let (shutdown_sender, _) = broadcast::channel(1);

        let wake_up_notify = Arc::new(Notify::new());
        // Immediately notify so we can start processing if we have pending identities
        // in the database
        wake_up_notify.notify_one();

        let pending_batch_submissions_queue = AsyncQueue::new(self.pending_identities_capacity);
        let mined_roots_queue = AsyncQueue::new(self.mined_roots_capacity);

        let mut handles = Vec::new();

        // Finalize identities task
        let finalize_identities = FinalizeRoots::new(
            self.database.clone(),
            self.identity_manager.clone(),
            self.tree_state.get_mined_tree(),
            self.finalization_max_attempts,
            Duration::from_secs(self.finalization_sleep_time_seconds),
        );

        let finalize_identities_handle = crate::utils::spawn_monitored_with_backoff(
            move || finalize_identities.clone().run(),
            shutdown_sender.clone(),
            FINALIZE_IDENTITIES_BACKOFF,
        );

        handles.push(finalize_identities_handle);

        // Mine identities task
        let mine_identities = MineIdentities::new(
            self.database.clone(),
            self.identity_manager.clone(),
            self.tree_state.get_processed_tree(),
            pending_batch_submissions_queue.clone(),
            mined_roots_queue,
        );

        let mine_identities_handle = crate::utils::spawn_monitored_with_backoff(
            move || mine_identities.clone().run(),
            shutdown_sender.clone(),
            MINE_IDENTITIES_BACKOFF,
        );

        handles.push(mine_identities_handle);

        // Prcess identities task
        let process_identities = ProcessIdentities::new(
            self.database.clone(),
            self.identity_manager.clone(),
            self.tree_state.get_batching_tree(),
            self.batch_insert_timeout_secs,
            pending_batch_submissions_queue,
            wake_up_notify.clone(),
        );

        let process_identities_handle = crate::utils::spawn_monitored_with_backoff(
            move || process_identities.clone().run(),
            shutdown_sender.clone(),
            PROCESS_IDENTITIES_BACKOFF,
        );

        handles.push(process_identities_handle);

        // Insert identities task
        let insert_identities = InsertIdentities::new(
            self.database.clone(),
            self.tree_state.get_latest_tree(),
            wake_up_notify,
        );

        let insert_identities_handle = crate::utils::spawn_monitored_with_backoff(
            move || insert_identities.clone().run(),
            shutdown_sender.clone(),
            INSERT_IDENTITIES_BACKOFF,
        );

        handles.push(insert_identities_handle);

        *instance = Some(RunningInstance {
            handles,
            shutdown_sender,
        });
    }

    async fn log_pending_identities_count(database: &Database) -> AnyhowResult<()> {
        let identities = database.count_pending_identities().await?;
        PENDING_IDENTITIES.set(f64::from(identities));
        Ok(())
    }

    async fn log_unprocessed_identities_count(database: &Database) -> AnyhowResult<()> {
        let identities = database.count_unprocessed_identities().await?;
        UNPROCESSED_IDENTITIES.set(f64::from(identities));
        Ok(())
    }

    async fn log_identities_queues(database: &Database) -> AnyhowResult<()> {
        TaskMonitor::log_unprocessed_identities_count(database).await?;
        TaskMonitor::log_pending_identities_count(database).await?;
        Ok(())
    }

    #[allow(clippy::cast_precision_loss)]
    fn log_batch_size(size: usize) {
        BATCH_SIZES.observe(size as f64);
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
