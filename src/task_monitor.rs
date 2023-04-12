use std::{sync::Arc, time::Duration};

use anyhow::Result as AnyhowResult;
use clap::Parser;
use ethers::types::U256;
use once_cell::sync::Lazy;
use prometheus::{linear_buckets, register_gauge, register_histogram, Gauge, Histogram};
use tokio::{
    sync::{broadcast, mpsc, Mutex, Notify, RwLock},
    task::JoinHandle,
};
use tracing::{info, instrument, warn};

use self::tasks::{
    insert_identities::{IdentityInsert, InsertIdentities},
    mine_identities::MineIdentities,
    process_identities::ProcessIdentities,
};
use crate::{
    contracts::SharedIdentityManager, database::Database, ethereum::write::TransactionId,
    identity_tree::TreeState,
};

pub mod tasks;

const PROCESS_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const MINE_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const INSERT_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);

struct RunningInstance {
    handles:         Vec<JoinHandle<()>>,
    shutdown_sender: broadcast::Sender<()>,
}

#[derive(Debug, Clone)]
pub struct PendingIdentities {
    identity_keys:  Vec<usize>,
    transaction_id: TransactionId,
    pre_root:       U256,
    post_root:      U256,
    start_index:    usize,
}

static PENDING_IDENTITIES: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!("pending_identities", "Identities not submitted on-chain").unwrap()
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

    /// How many identities can be held in the API insertion queue at any given
    /// time Past this limit the API request will block until the queue has
    /// space for the insertion.
    #[clap(long, env, default_value = "100")]
    pub insert_identities_capacity: usize,

    /// How many transactions can be sent "at once" to the blockchain via the
    /// write provider.
    ///
    /// It's not recommended to use values other than 1 without the
    /// `oz-provider` feature enabled.
    #[clap(long, env, default_value = "1")]
    pub pending_identities_capacity: usize,
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
    instance:                   RwLock<Option<RunningInstance>>,
    database:                   Arc<Database>,
    identity_manager:           SharedIdentityManager,
    tree_state:                 TreeState,
    batch_insert_timeout_secs:  u64,
    insert_identities_capacity: usize,
}

impl TaskMonitor {
    pub fn new(
        database: Arc<Database>,
        contracts: SharedIdentityManager,
        tree_state: TreeState,
        options: &Options,
    ) -> Self {
        let batch_insert_timeout_secs = options.batch_timeout_seconds;
        let insert_identities_capacity = options.insert_identities_capacity;
        Self {
            instance: RwLock::new(None),
            database,
            identity_manager: contracts,
            tree_state,
            batch_insert_timeout_secs,
            insert_identities_capacity,
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn start(&self) -> mpsc::Sender<IdentityInsert> {
        let mut instance = self.instance.write().await;
        if instance.is_some() {
            warn!("Identity committer already running");
        }

        // We could use the second element of the tuple as `mut shutdown_receiver`,
        // but for symmetry's sake we create it for every task with `.subscribe()`
        let (shutdown_sender, _) = broadcast::channel(1);
        let (pending_identities_sender, pending_identities_receiver) = mpsc::channel(2);
        let (insert_identities_sender, insert_identities_receiver) =
            mpsc::channel(self.insert_identities_capacity);

        let wake_up_notify = Arc::new(Notify::new());
        // Immediately notify so we can start processing if we have pending identities
        // in the database
        wake_up_notify.notify_one();

        // We need to maintain mutable access to these receivers from multiple
        // invocations of this task
        let pending_identities_receiver = Arc::new(Mutex::new(pending_identities_receiver));
        let insert_identities_receiver = Arc::new(Mutex::new(insert_identities_receiver));

        let mut handles = Vec::new();

        // Mine identities task
        let mine_identities = MineIdentities::new(
            self.database.clone(),
            self.identity_manager.clone(),
            self.tree_state.get_mined_tree(),
            pending_identities_receiver,
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
            pending_identities_sender,
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
            insert_identities_receiver,
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

        insert_identities_sender
    }

    async fn log_pending_identities_count(database: &Database) -> AnyhowResult<()> {
        let pending_identities = database.count_pending_identities().await?;
        PENDING_IDENTITIES.set(f64::from(pending_identities));
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
