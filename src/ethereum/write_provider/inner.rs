use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::H256;

use crate::ethereum::TxError;
use crate::identity::processor::TransactionId;

#[async_trait::async_trait]
pub trait Inner: Send + Sync + 'static {
    async fn send_transaction(
        &self,
        tx: TypedTransaction,
        only_once: bool,
        tx_id: Option<String>,
    ) -> Result<TransactionId, TxError>;

    async fn mine_transaction(&self, tx: TransactionId) -> Result<TransactionResult, TxError>;
}

pub struct TransactionResult {
    pub transaction_id: String,
    pub hash: Option<H256>,
}
