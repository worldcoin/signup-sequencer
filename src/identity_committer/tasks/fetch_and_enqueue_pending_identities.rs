use anyhow::Result as AnyhowResult;
use tokio::sync::{mpsc, oneshot};

use crate::{
    contracts::IdentityManager,
    identity_committer::{IdentityCommitter, PendingIdentities},
    identity_tree::TreeVersion,
};

impl IdentityCommitter {
    pub async fn fetch_and_enqueue_pending_identities(
        sender: oneshot::Sender<()>,
        batching_tree: &TreeVersion,
        identity_manager: &IdentityManager,
        pending_identities_sender: &mpsc::Sender<PendingIdentities>,
    ) -> AnyhowResult<()> {
        let mut pending_identities = identity_manager.fetch_pending_identities().await?;

        if pending_identities.is_empty() {
            return Ok(());
        }

        if pending_identities.len() > 1 {
            return Err(anyhow::anyhow!(
                "More than one pending identity is not supported yet"
            ));
        }

        let (transaction_id, register_identities_call) = pending_identities.remove(0);

        let num_updates = register_identities_call.identity_commitments.len();

        let updates = batching_tree.peek_next_updates(num_updates).await;

        if updates.is_empty() {
            return Err(anyhow::anyhow!("No updates available"));
        }

        batching_tree.apply_next_updates(updates.len()).await;

        let start_index = updates[0].leaf_index;
        let identity_keys: Vec<usize> = updates.iter().map(|update| update.leaf_index).collect();

        let pre_root = register_identities_call.pre_root;
        let post_root = register_identities_call.post_root;

        pending_identities_sender
            .send(PendingIdentities {
                identity_keys,
                transaction_id,
                pre_root,
                post_root,
                start_index,
            })
            .await?;

        let _ = sender.send(());

        Ok(())
    }
}
