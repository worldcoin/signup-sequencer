use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use once_cell::sync::Lazy;
use prometheus::{linear_buckets, register_gauge, register_histogram, Gauge, Histogram};
use tokio::sync::{broadcast, mpsc, Mutex, Notify, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, instrument, warn};

use self::tasks::delete_identities::DeleteIdentities;
use self::tasks::finalize_identities::FinalizeRoots;
use self::tasks::insert_identities::InsertIdentities;
use self::tasks::monitor_txs::MonitorTxs;
use self::tasks::process_identities::ProcessIdentities;
use crate::contracts::SharedIdentityManager;
use crate::database::Database;
use crate::identity_tree::TreeState;

pub mod tasks;

const PROCESS_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const FINALIZE_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const INSERT_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const DELETE_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);

struct RunningInstance {
    handles:         Vec<JoinHandle<()>>,
    shutdown_sender: broadcast::Sender<()>,
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
    async fn shutdown(self) -> anyhow::Result<()> {
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
    // TODO: do we want to change this to batch_insertion_timeout_secs
    #[clap(long, env, default_value = "180")]
    pub batch_timeout_seconds: u64,

    /// TODO:
    #[clap(long, env, default_value = "3600")]
    pub batch_deletion_timeout_seconds: i64,

    /// TODO:
    #[clap(long, env, default_value = "100")]
    pub min_batch_deletion_size: usize,

    /// The parameter to control the delay between mining a deletion batch and
    /// inserting the recovery identities
    ///
    /// The sequencer will insert the recovery identities after
    /// max_epoch_duration_seconds + root_history_expiry) seconds have passed
    ///
    /// By default the value is set to 0 so the sequencer will only use
    /// root_history_expiry
    #[clap(long, env, default_value = "0")]
    pub max_epoch_duration_seconds: u64,

    /// The maximum number of windows to scan for finalization logs
    #[clap(long, env, default_value = "100")]
    pub scanning_window_size: u64,

    /// The offset from the latest block to scan
    #[clap(long, env, default_value = "0")]
    pub scanning_chain_head_offset: u64,

    /// The number of seconds to wait between fetching logs
    #[clap(long, env, default_value = "30")]
    pub time_between_scans_seconds: u64,

    /// The number of txs in the channel that we'll be monitoring
    #[clap(long, env, default_value = "100")]
    pub monitored_txs_capacity: usize,
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
    instance:                  RwLock<Option<RunningInstance>>,
    database:                  Arc<Database>,
    identity_manager:          SharedIdentityManager,
    tree_state:                TreeState,
    batch_insert_timeout_secs: u64,

    // Finalization params
    scanning_window_size:           u64,
    scanning_chain_head_offset:     u64,
    time_between_scans:             Duration,
    max_epoch_duration:             Duration,
    // TODO: docs
    batch_deletion_timeout_seconds: i64,
    // TODO: docs
    min_batch_deletion_size:        usize,
    monitored_txs_capacity:         usize,
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
            scanning_window_size,
            scanning_chain_head_offset,
            time_between_scans_seconds,
            max_epoch_duration_seconds,
            monitored_txs_capacity,
            batch_deletion_timeout_seconds,
            min_batch_deletion_size,
        } = *options;

        Self {
            instance: RwLock::new(None),
            database,
            identity_manager: contracts,
            tree_state,
            batch_insert_timeout_secs: batch_timeout_seconds,
            scanning_window_size,
            scanning_chain_head_offset,
            time_between_scans: Duration::from_secs(time_between_scans_seconds),
            batch_deletion_timeout_seconds,
            min_batch_deletion_size,
            max_epoch_duration: Duration::from_secs(max_epoch_duration_seconds),
            monitored_txs_capacity,
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

        let (monitored_txs_sender, monitored_txs_receiver) =
            mpsc::channel(self.monitored_txs_capacity);

        let wake_up_notify = Arc::new(Notify::new());
        // Immediately notify so we can start processing if we have pending identities
        // in the database
        wake_up_notify.notify_one();

        let pending_insertion_mutex = Arc::new(Mutex::new(()));

        let mut handles = Vec::new();

        // Finalize identities task
        let finalize_identities = FinalizeRoots::new(
            self.database.clone(),
            self.identity_manager.clone(),
            self.tree_state.get_processed_tree(),
            self.tree_state.get_mined_tree(),
            self.scanning_window_size,
            self.scanning_chain_head_offset,
            self.time_between_scans,
            self.max_epoch_duration,
        );

        let finalize_identities_handle = crate::utils::spawn_monitored_with_backoff(
            move || finalize_identities.clone().run(),
            shutdown_sender.clone(),
            FINALIZE_IDENTITIES_BACKOFF,
        );

        handles.push(finalize_identities_handle);

        // Process identities task
        let process_identities = ProcessIdentities::new(
            self.database.clone(),
            self.identity_manager.clone(),
            self.tree_state.get_batching_tree(),
            self.batch_insert_timeout_secs,
            monitored_txs_sender,
            wake_up_notify.clone(),
        );

        let process_identities_handle = crate::utils::spawn_monitored_with_backoff(
            move || process_identities.clone().run(),
            shutdown_sender.clone(),
            PROCESS_IDENTITIES_BACKOFF,
        );

        handles.push(process_identities_handle);

        let monitor_txs = MonitorTxs::new(self.identity_manager.clone(), monitored_txs_receiver);

        let monitor_txs_handle = crate::utils::spawn_monitored_with_backoff(
            move || monitor_txs.clone().run(),
            shutdown_sender.clone(),
            PROCESS_IDENTITIES_BACKOFF,
        );

        handles.push(monitor_txs_handle);

        // Insert identities task
        let insert_identities = InsertIdentities::new(
            self.database.clone(),
            self.tree_state.get_latest_tree(),
            wake_up_notify.clone(),
            pending_insertion_mutex.clone(),
        );

        let insert_identities_handle = crate::utils::spawn_monitored_with_backoff(
            move || insert_identities.clone().run(),
            shutdown_sender.clone(),
            INSERT_IDENTITIES_BACKOFF,
        );

        handles.push(insert_identities_handle);

        // Delete identities task
        let delete_identities = DeleteIdentities::new(
            self.database.clone(),
            self.tree_state.get_latest_tree(),
            self.batch_deletion_timeout_seconds,
            self.min_batch_deletion_size,
            wake_up_notify,
            pending_insertion_mutex,
        );

        let delete_identities_handle = crate::utils::spawn_monitored_with_backoff(
            move || delete_identities.clone().run(),
            shutdown_sender.clone(),
            DELETE_IDENTITIES_BACKOFF,
        );

        handles.push(delete_identities_handle);

        *instance = Some(RunningInstance {
            handles,
            shutdown_sender,
        });
    }

    async fn log_pending_identities_count(database: &Database) -> anyhow::Result<()> {
        let identities = database.count_pending_identities().await?;
        PENDING_IDENTITIES.set(f64::from(identities));
        Ok(())
    }

    async fn log_unprocessed_identities_count(database: &Database) -> anyhow::Result<()> {
        let identities = database.count_unprocessed_identities().await?;
        UNPROCESSED_IDENTITIES.set(f64::from(identities));
        Ok(())
    }

    async fn log_identities_queues(database: &Database) -> anyhow::Result<()> {
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
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        let mut instance = self.instance.write().await;
        if let Some(instance) = instance.take() {
            instance.shutdown().await?;
        } else {
            info!("Committer not running.");
        }
        Ok(())
    }
}
