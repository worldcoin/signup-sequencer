use std::error::Error;

use alloy::transports::TransportError;
use alloy::{primitives::B256, rpc::types::TransactionReceipt};
use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)] // Unused variants
pub enum TxError {
    #[error("Error filling transaction: {0}")]
    Fill(Box<dyn Error + Send + Sync + 'static>),

    #[error("Error fetching transaction from the blockchain: {0}")]
    Fetch(Box<dyn Error + Send + Sync + 'static>),

    #[error("Timeout while sending transaction")]
    SendTimeout,

    #[error("Error sending transaction: {0:?}")]
    Send(anyhow::Error),

    #[error("Timeout while waiting for confirmations")]
    ConfirmationTimeout,

    #[error("Error waiting for confirmations: {0}")]
    Confirmation(TransportError),

    #[error("Transaction dropped from mempool: {0}.")]
    Dropped(B256),

    #[error("Transaction failed: {0:?}.")]
    Failed(Box<Option<TransactionReceipt>>),

    #[error("Error parsing transaction id: {0}")]
    Parse(Box<dyn Error + Send + Sync + 'static>),

    #[error("{0:?}")]
    Other(anyhow::Error),
}
