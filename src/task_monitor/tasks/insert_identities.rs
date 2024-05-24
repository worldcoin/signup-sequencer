use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::time::sleep;
use tracing::instrument;

use crate::app::App;
use crate::database::query::DatabaseQuery as _;
use crate::database::types::UnprocessedCommitment;
use crate::database::Database;
use crate::identity_tree::{Latest, TreeVersion, TreeVersionReadOps, UnprocessedStatus};

pub async fn insert_identities(
    app: Arc<App>,
    pending_insertions_mutex: Arc<Mutex<()>>,
    wake_up_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    loop {
        // get commits from database
        let unprocessed = app
            .database
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?;
        if unprocessed.is_empty() {
            sleep(Duration::from_secs(5)).await;
            continue;
        }

        insert_identities_batch(
            &app.database,
            app.tree_state()?.latest_tree(),
            unprocessed,
            &pending_insertions_mutex,
        )
        .await?;
        // Notify the identity processing task, that there are new identities
        wake_up_notify.notify_one();
    }
}

#[instrument(level = "info", skip_all)]
async fn insert_identities_batch(
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

    let _guard = pending_insertions_mutex.lock().await;

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

    for ((root, _proof, leaf_index), identity) in items {
        database
            .insert_pending_identity(leaf_index, &identity, &root)
            .await?;

        database.remove_unprocessed_identity(&identity).await?;
    }

    Ok(())
}
