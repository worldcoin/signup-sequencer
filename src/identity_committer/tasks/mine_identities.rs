use anyhow::Result as AnyhowResult;
use tokio::sync::mpsc;
use tracing::{info, instrument, warn};

use crate::{
    contracts::IdentityManager,
    database::Database,
    identity_committer::{IdentityCommitter, PendingIdentities},
    identity_tree::TreeVersion,
};

impl IdentityCommitter {
    #[instrument(level = "info", skip_all)]
    pub async fn mine_identities(
        database: &Database,
        identity_manager: &IdentityManager,
        mined_tree: &TreeVersion,
        mut pending_identities_receiver: mpsc::Receiver<PendingIdentities>,
    ) -> AnyhowResult<()> {
        loop {
            let Some(pending_identity) = pending_identities_receiver.recv().await else {
            warn!("Pending identities channel closed, terminating.");
            break;
        };

            let PendingIdentities {
                identity_keys,
                transaction_id,
                pre_root,
                post_root,
                start_index,
            } = pending_identity;

            identity_manager.mine_identities(transaction_id).await?;

            // With this done, all that remains is to mark them as submitted to the
            // blockchain in the source-of-truth database, and also update the mined tree to
            // agree with the database and chain.
            database
                .mark_identities_submitted_to_contract(&post_root.into(), identity_keys.as_slice())
                .await?;

            info!(start_index, ?pre_root, ?post_root, "Batch mined");

            mined_tree.apply_next_updates(identity_keys.len()).await;

            Self::log_pending_identities_count(database).await?;
        }
        Ok(())
    }
}
