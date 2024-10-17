use std::sync::Arc;
use std::time::Duration;

use sqlx::{Postgres, Transaction};
use tokio::sync::{Mutex, Notify};
use tokio::time;
use tracing::info;

use crate::app::App;
use crate::database::methods::DbMethods as _;
use crate::database::types::UnprocessedCommitment;
use crate::identity_tree::{Latest, TreeVersion, TreeVersionReadOps, UnprocessedStatus};
use crate::retry_tx;

// Insertion here differs from delete_identities task. This is because two
// different flows are created for both tasks. We need to insert identities as
// fast as possible to the tree to be able to return inclusion proof as our
// customers depend on it. Flow here is to rewrite from unprocessed_identities
// into identities every 5 seconds.
pub async fn insert_identities(
    app: Arc<App>,
    pending_insertions_mutex: Arc<Mutex<()>>,
    wake_up_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    info!("Starting insertion processor task.");

    let mut timer = time::interval(Duration::from_secs(5));

    loop {
        _ = timer.tick().await;
        info!("Insertion processor woken due to timeout.");

        // get commits from database
        let unprocessed = app
            .database
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?;
        if unprocessed.is_empty() {
            continue;
        }

        let _guard = pending_insertions_mutex.lock().await;
        let latest_tree = app.tree_state()?.latest_tree();

        retry_tx!(&app.database, tx, {
            insert_identities_batch(&mut tx, latest_tree, &unprocessed).await
        })
        .await?;

        // Notify the identity processing task, that there are new identities
        wake_up_notify.notify_one();
    }
}

pub async fn insert_identities_batch(
    tx: &mut Transaction<'_, Postgres>,
    latest_tree: &TreeVersion<Latest>,
    identities: &[UnprocessedCommitment],
) -> Result<(), anyhow::Error> {
    // Filter out any identities that are already in the `identities` table
    let mut filtered_identities = vec![];
    for identity in identities {
        if tx
            .get_identity_leaf_index(&identity.commitment)
            .await?
            .is_some()
        {
            tracing::warn!(?identity.commitment, "Duplicate identity");
            tx.remove_unprocessed_identity(&identity.commitment).await?;
        } else {
            filtered_identities.push(identity.commitment);
        }
    }

    let next_leaf = latest_tree.next_leaf();

    let next_db_index = tx.get_next_leaf_index().await?;

    assert_eq!(
        next_leaf, next_db_index,
        "Database and tree are out of sync. Next leaf index in tree is: {next_leaf}, in database: \
         {next_db_index}"
    );

    let mut pre_root = &latest_tree.get_root();
    let data = latest_tree.append_many(&filtered_identities);

    assert_eq!(
        data.len(),
        filtered_identities.len(),
        "Length mismatch when appending identities to tree"
    );

    for ((root, _proof, leaf_index), identity) in data.iter().zip(&filtered_identities) {
        tx.insert_pending_identity(*leaf_index, identity, root, pre_root)
            .await?;
        pre_root = root;

        tx.remove_unprocessed_identity(identity).await?;
    }

    Ok(())
}
