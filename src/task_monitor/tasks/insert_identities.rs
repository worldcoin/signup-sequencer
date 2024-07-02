use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::time;
use tracing::info;

use crate::app::App;
use crate::database::query::DatabaseQuery as _;
use crate::database::types::UnprocessedCommitment;
use crate::database::Database;
use crate::identity_tree::{Latest, TreeVersion, TreeVersionReadOps, UnprocessedStatus};
use crate::retry_tx;

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

        insert_identities_batch(
            &app.database,
            app.tree_state()?.latest_tree(),
            &unprocessed,
            &pending_insertions_mutex,
        )
        .await?;

        // Notify the identity processing task, that there are new identities
        wake_up_notify.notify_one();
    }
}

pub async fn insert_identities_batch(
    database: &Database,
    latest_tree: &TreeVersion<Latest>,
    identities: &[UnprocessedCommitment],
    pending_insertions_mutex: &Mutex<()>,
) -> anyhow::Result<()> {
    let filtered_identities = retry_tx!(database, tx, {
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
        Result::<_, anyhow::Error>::Ok(filtered_identities)
    })
    .await?;

    let _guard = pending_insertions_mutex.lock().await;

    let next_leaf = latest_tree.next_leaf();

    let next_db_index = retry_tx!(database, tx, tx.get_next_leaf_index().await).await?;

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

    retry_tx!(database, tx, {
        for ((root, _proof, leaf_index), identity) in data.iter().zip(&filtered_identities) {
            tx.insert_pending_identity(*leaf_index, identity, root)
                .await?;

            tx.remove_unprocessed_identity(identity).await?;
        }

        Result::<_, anyhow::Error>::Ok(())
    })
    .await
}
