use std::sync::Arc;
use std::time::Duration;

use tokio::{pin, sync::{Mutex, Notify}};
use tokio::time;
use tracing::info;

use crate::app::App;
use crate::database::methods::DbMethods as _;
use crate::database::IsolationLevel;
use crate::identity_tree::{BasicTreeOps, UnprocessedStatus};

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

        let mut tx = app.database.begin_tx(IsolationLevel::ReadCommitted).await?;

        // Locks the tree for the duration of the operation
        let mut tree = latest_tree.get_data();
        pin!(tree);

        let mut pre_root = tree.root();

        let next_leaf = tree.next_leaf();
        let next_db_index = tx.get_next_leaf_index().await?;
        assert_eq!(
            next_leaf, next_db_index,
            "Database and tree are out of sync. Next leaf index in tree is: {next_leaf}, in database: {next_db_index}"
        );

        for (idx, identity) in unprocessed.iter().enumerate() {
            let leaf_idx = next_leaf + idx;
            tree.update(leaf_idx, identity.commitment);
            let root = tree.root();

            tx.insert_pending_identity(leaf_idx, &identity.commitment, &root, &pre_root)
                .await?;

            pre_root = root;
        }

        tx.trim_unprocessed().await?;

        tx.commit().await?;

        // Notify the identity processing task, that there are new identities
        wake_up_notify.notify_one();
    }
}
