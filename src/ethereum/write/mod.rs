use ethers::{
    providers::ProviderError,
    types::{TransactionReceipt, H256},
};
use std::error::Error;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TxError {
    #[error("Error filling transaction: {0}")]
    Fill(Box<dyn Error + Send + Sync + 'static>),

    #[error("Timeout while sending transaction")]
    SendTimeout,

    #[error("Error sending transaction: {0}")]
    Send(Box<dyn Error + Send + Sync + 'static>),

    #[error("Timeout while waiting for confirmations")]
    ConfirmationTimeout,

    #[error("Error waiting for confirmations: {0}")]
    Confirmation(ProviderError),

    #[error("Transaction dropped from mempool.")]
    Dropped(H256),

    #[error("Transaction failed.")]
    Failed(Box<TransactionReceipt>),
}
