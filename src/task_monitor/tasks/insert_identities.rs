use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result as AnyhowResult;
use tokio::sync::Notify;
use tokio::time::sleep;
use tracing::instrument;

use crate::database::types::UnprocessedCommitment;
use crate::database::Database;
use crate::identity_tree::{Hash, Latest, TreeVersion, TreeVersionReadOps, UnprocessedStatus};

pub struct InsertIdentities {
    database:       Arc<Database>,
    latest_tree:    TreeVersion<Latest>,
    wake_up_notify: Arc<Notify>,
}

impl InsertIdentities {
    pub fn new(
        database: Arc<Database>,
        latest_tree: TreeVersion<Latest>,
        wake_up_notify: Arc<Notify>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            latest_tree,
            wake_up_notify,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        insert_identities_loop(&self.database, &self.latest_tree, &self.wake_up_notify).await
    }
}

async fn insert_identities_loop(
    database: &Database,
    latest_tree: &TreeVersion<Latest>,
    wake_up_notify: &Notify,
) -> AnyhowResult<()> {
    loop {
        // get commits from database
        let unprocessed = database
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?;
        if unprocessed.is_empty() {
            sleep(Duration::from_secs(5)).await;
            continue;
        }

        insert_identities(database, latest_tree, unprocessed).await?;
        // Notify the identity processing task, that there are new identities
        wake_up_notify.notify_one();
    }
}

#[instrument(level = "info", skip_all)]
async fn insert_identities(
    database: &Database,
    latest_tree: &TreeVersion<Latest>,
    identities: Vec<UnprocessedCommitment>,
) -> AnyhowResult<()> {
    let next_db_index = database.get_next_leaf_index().await?;
    let next_leaf = latest_tree.next_leaf();

    assert_eq!(
        next_leaf, next_db_index,
        "Database and tree are out of sync. Next leaf index in tree is: {next_leaf}, in database: \
         {next_db_index}"
    );

    let identities: Vec<Hash> = identities
        .into_iter()
        .map(|insert| insert.commitment)
        .collect();

    let data = latest_tree.append_many(&identities);

    assert_eq!(
        data.len(),
        identities.len(),
        "Length mismatch when appending identities to tree"
    );

    let items = data.into_iter().zip(identities);

    for ((root, _proof, leaf_index), identity) in items {
        database
            .insert_pending_identity(leaf_index, &identity, &root)
            .await?;

        database.remove_unprocessed_identity(&identity).await?;
    }

    Ok(())
}
