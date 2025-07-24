use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::U256;
use reqwest::StatusCode;
use tx_sitter_client::data::{SendTxRequest, TransactionPriority, TxStatus};
use tx_sitter_client::{ErrorResponse, TxSitterClient};

use super::inner::{Inner, TransactionResult};
use crate::config::TxSitterConfig;
use crate::ethereum::TxError;
use crate::identity::processor::TransactionId;

const MINING_TIMEOUT: Duration = Duration::from_secs(60);

pub struct TxSitter {
    client: TxSitterClient,
    gas_limit: Option<u64>,
}

impl TxSitter {
    pub fn new(config: &TxSitterConfig) -> Self {
        Self {
            client: TxSitterClient::new(&config.tx_sitter_url),
            gas_limit: config.tx_sitter_gas_limit,
        }
    }

    pub async fn mine_transaction_inner(
        &self,
        tx_id: TransactionId,
    ) -> Result<TransactionResult, TxError> {
        loop {
            let tx = self
                .client
                .get_tx(&tx_id)
                .await
                .context("Error fetching tx")
                .map_err(TxError::Send)?;

            if tx.status == Some(TxStatus::Mined) || tx.status == Some(TxStatus::Finalized) {
                return Ok(TransactionResult {
                    transaction_id: tx.tx_id,
                    hash: Some(
                        tx.tx_hash
                            .context("Missing hash on a mined tx")
                            .map_err(TxError::Send)?,
                    ),
                });
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

#[async_trait]
impl Inner for TxSitter {
    async fn send_transaction(
        &self,
        mut tx: TypedTransaction,
        _only_once: bool,
        tx_id: Option<String>,
    ) -> Result<TransactionId, TxError> {
        if let Some(gas_limit) = self.gas_limit {
            tx.set_gas(gas_limit);
        }

        // TODO: Handle only_once
        let res = self
            .client
            .send_tx(&SendTxRequest {
                to: *tx
                    .to_addr()
                    .context("Tx receiver must be an address")
                    .map_err(TxError::Send)?,
                value: tx.value().copied().unwrap_or(U256::zero()),
                data: tx.data().cloned(),
                gas_limit: *tx
                    .gas()
                    .context("Missing tx gas limit")
                    .map_err(TxError::Send)?,
                priority: TransactionPriority::Regular,
                tx_id: tx_id.clone(),
            })
            .await;

        let res = match res {
            Err(err) => match err.downcast_ref::<ErrorResponse>() {
                Some(resp_err)
                    if resp_err.status == StatusCode::CONFLICT
                        && resp_err.body.error_id == "transaction_already_exists" =>
                {
                    if let Some(tx_id) = tx_id {
                        return Ok(tx_id);
                    }
                    Err(err)
                }
                _ => Err(err),
            },
            res => res,
        };

        let tx = res
            .context("Error sending transaction")
            .map_err(TxError::Send)?;

        Ok(tx.tx_id)
    }

    async fn mine_transaction(&self, tx: TransactionId) -> Result<TransactionResult, TxError> {
        tokio::time::timeout(MINING_TIMEOUT, self.mine_transaction_inner(tx))
            .await
            .map_err(|_| TxError::ConfirmationTimeout)?
    }
}
