use anyhow::Context;
use async_trait::async_trait;
use ethers::types::transaction::eip2718::TypedTransaction;
use tx_sitter_client::data::SendTxRequest;
use tx_sitter_client::TxSitterClient;

use super::inner::{Inner, TransactionResult};
use crate::ethereum::write::TransactionId;
use crate::ethereum::TxError;

pub struct TxSitter {
    client: TxSitterClient,
}

impl TxSitter {
    pub fn new(url: impl ToString) -> Self {
        Self {
            client: TxSitterClient::new(url),
        }
    }
}

#[async_trait]
impl Inner for TxSitter {
    async fn send_transaction(
        &self,
        tx: TypedTransaction,
        only_once: bool,
    ) -> Result<TransactionId, TxError> {
        let x = self.client.send_tx(&SendTxRequest {
            to: *tx.to_addr().context("Tx receiver must be an address")?,
            value:     *tx.value().context("Missing tx value")?,
            data:      todo!(),
            gas_limit: todo!(),
            priority:  todo!(),
            tx_id:     todo!(),
        }).await.unwrap();

        todo!()
    }

    async fn fetch_pending_transactions(&self) -> Result<Vec<TransactionId>, TxError> {
        todo!()
    }

    async fn mine_transaction(&self, tx: TransactionId) -> Result<TransactionResult, TxError> {
        todo!()
    }
}
