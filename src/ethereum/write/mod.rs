use std::error::Error;
use std::fmt;

use async_trait::async_trait;
use ethers::providers::ProviderError;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, TransactionReceipt, H256};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct TransactionId(pub String);

impl AsRef<str> for TransactionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransactionId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Error)]
#[allow(dead_code)] // Unused variants
pub enum TxError {
    #[error("Error filling transaction: {0}")]
    Fill(Box<dyn Error + Send + Sync + 'static>),

    #[error("Error fetching transaction from the blockchain: {0}")]
    Fetch(Box<dyn Error + Send + Sync + 'static>),

    #[error("Timeout while sending transaction")]
    SendTimeout,

    #[error("Error sending transaction: {0}")]
    Send(Box<dyn Error + Send + Sync + 'static>),

    #[error("Timeout while waiting for confirmations")]
    ConfirmationTimeout,

    #[error("Error waiting for confirmations: {0}")]
    Confirmation(ProviderError),

    #[error("Transaction dropped from mempool: {0}.")]
    Dropped(H256),

    #[error("Transaction failed: {0:?}.")]
    Failed(Option<TransactionReceipt>),

    #[error("Error parsing transaction id: {0}")]
    Parse(Box<dyn Error + Send + Sync + 'static>),
}
