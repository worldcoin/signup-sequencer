use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result as AnyhowResult;
use ethers::types::U256;
use tokio::sync::Notify;
use tokio::{select, time};
use tracing::{info, instrument};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{Canonical, Intermediate, TreeVersion, TreeWithNextVersion};
use crate::task_monitor::PendingBatchSubmission;
use crate::utils::async_queue::{AsyncPopGuard, AsyncQueue};

// It can take up to 40 minutes to bridge the root

pub struct DeleteIdentities {
    database: Arc<Database>,
    identity_manager: SharedIdentityManager,
    batching_tree: TreeVersion<Intermediate>,
    batch_insert_timeout_secs: u64,
    pending_batch_submissions_queue: AsyncQueue<PendingBatchSubmission>,
    wake_up_notify: Arc<Notify>,
}

impl DeleteIdentities {
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
        delete_identities(
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

async fn delete_identities(
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
        select! {
            _ = timer.tick() => {


            }
            _ = wake_up_notify.notified() => {

            }
        }
    }
}
