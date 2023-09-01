use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result as AnyhowResult;
use ethers::types::U256;
use tokio::sync::Notify;
use tokio::{select, time};
use tracing::{info, instrument};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::types::DeletionEntry;
use crate::database::Database;
use crate::identity_tree::{
    Canonical, Hash, Intermediate, Latest, TreeVersion, TreeVersionReadOps, TreeWithNextVersion,
};
use crate::task_monitor::PendingBatchSubmission;
use crate::utils::async_queue::{AsyncPopGuard, AsyncQueue};

// It can take up to 40 minutes to bridge the root

pub struct DeleteIdentities {
    database:         Arc<Database>,
    identity_manager: SharedIdentityManager,
    latest_tree:      TreeVersion<Latest>,

    wake_up_notify: Arc<Notify>,
}

impl DeleteIdentities {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        latest_tree: TreeVersion<Latest>,

        wake_up_notify: Arc<Notify>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            latest_tree,
            wake_up_notify,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        delete_identities(
            &self.database,
            &self.identity_manager,
            &self.latest_tree & self.wake_up_notify,
        )
        .await
    }
}

// TODO: maybe update to be &self
async fn delete_identities(
    database: &Database,
    identity_manager: &IdentityManager,
    wake_up_notify: &Notify,
    latest_tree: TreeVersion<Latest>,
    deletion_sleep_time: Duration,
) -> AnyhowResult<()> {
    // TODO: do we need to do this?
    info!("Awaiting for a clean slate");
    identity_manager.await_clean_slate().await?;

    info!("Starting identity processor.");

    // TODO: should this be max batch size for deletions
    let batch_size = identity_manager.max_batch_size().await;

    // We start a timer and force it to perform one initial tick to avoid an
    // immediate trigger.
    let mut timer = time::interval(deletion_sleep_time);
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

    // TODO: we need to track last deletion and update this, making sure that there
    // is a deletion every x interval

    loop {
        let deletions = database.get_deletions().await?;
        if deletions.is_empty() {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        // TODO: check batch size or time elapsed since last deletion

        // Dedup deletion entries
        let deletions = deletions
            .into_iter()
            .map(|f| f)
            .collect::<HashSet<DeletionEntry>>();

        // Validate the identities are not in the database

        let next_db_index = database.get_next_leaf_index().await?;
        let next_leaf = latest_tree.next_leaf();

        assert_eq!(
            next_leaf, next_db_index,
            "Database and tree are out of sync. Next leaf index in tree is: {next_leaf}, in \
             database: {next_db_index}"
        );

        let leaf_indices = deletions
            .iter()
            .map(|d| d.leaf_index)
            .collect::<Vec<usize>>();

        let data = latest_tree.delete_many(&leaf_indices);

        assert_eq!(
            data.len(),
            leaf_indices.len(),
            "Length mismatch when appending identities to tree"
        );

        let items = data.into_iter().zip(leaf_indices.into_iter());

        // TODO: insert into identities
        for ((root, _proof), leaf_index) in items {
            database
                .insert_pending_identity(leaf_index, &Hash::ZERO, &root)
                .await?;
        }

        // TODO: remove all deletions

        // Lock mutex

        // Get all of the deletions

        // Insert new identities into idt table

        // Update some tree ?

        // notify?
    }
}
