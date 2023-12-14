use async_trait::async_trait;
use ethers::types::transaction::eip2718::TypedTransaction;

use super::inner::{Inner, TransactionResult};
use crate::ethereum::write::TransactionId;
use crate::ethereum::TxError;

pub struct TxSitter {
    url: String,
}

#[async_trait]
impl Inner for TxSitter {
    async fn send_transaction(
        &self,
        tx: TypedTransaction,
        only_once: bool,
    ) -> Result<TransactionId, TxError> {
        todo!()
    }

    async fn fetch_pending_transactions(&self) -> Result<Vec<TransactionId>, TxError> {
        todo!()
    }

    async fn mine_transaction(&self, tx: TransactionId) -> Result<TransactionResult, TxError> {
        todo!()
    }
}
