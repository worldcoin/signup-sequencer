use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::time::sleep;
use tracing::instrument;

use crate::database::types::UnprocessedCommitment;
use crate::database::Database;
use crate::identity_tree::{Latest, TreeVersion, TreeVersionReadOps, UnprocessedStatus};

pub struct InsertIdentities {
    database:                 Arc<Database>,
    latest_tree:              TreeVersion<Latest>,
    wake_up_notify:           Arc<Notify>,
    pending_insertions_mutex: Arc<Mutex<()>>,
}

impl InsertIdentities {
    pub fn new(
        database: Arc<Database>,
        latest_tree: TreeVersion<Latest>,
        wake_up_notify: Arc<Notify>,
        pending_insertions_mutex: Arc<Mutex<()>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            latest_tree,
            wake_up_notify,
            pending_insertions_mutex,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        insert_identities_loop(
            &self.database,
            &self.latest_tree,
            &self.wake_up_notify,
            &self.pending_insertions_mutex,
        )
        .await
    }
}

async fn insert_identities_loop(
    database: &Database,
    latest_tree: &TreeVersion<Latest>,
    wake_up_notify: &Notify,
    pending_insertions_mutex: &Mutex<()>,
) -> anyhow::Result<()> {
    loop {
        // get commits from database
        let unprocessed = database
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?;
        if unprocessed.is_empty() {
            sleep(Duration::from_secs(5)).await;
            continue;
        }

        insert_identities(database, latest_tree, unprocessed, pending_insertions_mutex).await?;
        // Notify the identity processing task, that there are new identities
        wake_up_notify.notify_one();
    }
}

#[instrument(level = "info", skip_all)]
async fn insert_identities(
    database: &Database,
    latest_tree: &TreeVersion<Latest>,
    identities: Vec<UnprocessedCommitment>,
    pending_insertions_mutex: &Mutex<()>,
) -> anyhow::Result<()> {
    // Filter out any identities that are already in the `identities` table
    let mut filtered_identities = vec![];
    for identity in identities {
        if database
            .get_identity_leaf_index(&identity.commitment)
            .await?
            .is_some()
        {
            tracing::warn!(?identity.commitment, "Duplicate identity");
            database
                .remove_unprocessed_identity(&identity.commitment)
                .await?;
        } else {
            filtered_identities.push(identity.commitment);
        }
    }

    let next_db_index = database.get_next_leaf_index().await?;
    let next_leaf = latest_tree.next_leaf();

    assert_eq!(
        next_leaf, next_db_index,
        "Database and tree are out of sync. Next leaf index in tree is: {next_leaf}, in database: \
         {next_db_index}"
    );

    let data = latest_tree.append_many(&filtered_identities);

    assert_eq!(
        data.len(),
        filtered_identities.len(),
        "Length mismatch when appending identities to tree"
    );

    let items = data.into_iter().zip(filtered_identities);

    let _guard = pending_insertions_mutex.lock().await;

    for ((root, _proof, leaf_index), identity) in items {
        database
            .insert_pending_identity(leaf_index, &identity, &root)
            .await?;

        database.remove_unprocessed_identity(&identity).await?;
    }

    Ok(())
}
