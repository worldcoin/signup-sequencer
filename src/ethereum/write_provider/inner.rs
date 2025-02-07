use crate::ethereum::TxError;
use crate::identity::processor::TransactionId;
use alloy::consensus::TypedTransaction;
use alloy::primitives::B256;

#[async_trait::async_trait]
pub trait Inner: Send + Sync + 'static {
    async fn send_transaction(
        &self,
        tx: TypedTransaction,
        only_once: bool,
        tx_id: Option<String>,
    ) -> Result<TransactionId, TxError>;

    async fn fetch_pending_transactions(&self) -> Result<Vec<TransactionId>, TxError>;

    async fn mine_transaction(&self, tx: TransactionId) -> Result<TransactionResult, TxError>;
}

pub struct TransactionResult {
    pub transaction_id: String,
    pub hash: Option<B256>,
}
