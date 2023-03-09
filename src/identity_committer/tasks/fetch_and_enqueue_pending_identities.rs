use anyhow::Result as AnyhowResult;
use ethers::types::U256;
use tokio::sync::{mpsc, oneshot};

use crate::{
    contracts::{abi::RegisterIdentitiesCall, IdentityManager},
    ethereum::write::TransactionId,
    identity_committer::{IdentityCommitter, PendingIdentities},
    identity_tree::Hash,
};

type TransactionData = (TransactionId, RegisterIdentitiesCall);

impl IdentityCommitter {
    pub async fn fetch_and_enqueue_pending_identities(
        sender: oneshot::Sender<()>,
        identity_manager: &IdentityManager,
        pending_identities_sender: &mpsc::Sender<PendingIdentities>,
    ) -> AnyhowResult<()> {
        let mut pending_identities = identity_manager.fetch_pending_identities().await?;

        let _ = sender.send(());

        Ok(())
    }

    fn ordered_by_root_hashes(pending_identities: Vec<TransactionData>) -> Vec<TransactionData> {
        let mut ordered = Vec::with_capacity(pending_identities.len());

        ordered
    }

    fn root_hash_without_parent(data: &[TransactionData]) -> Option<U256> {
        None
    }
}
