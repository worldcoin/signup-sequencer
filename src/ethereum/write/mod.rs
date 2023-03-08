use async_trait::async_trait;
use ethers::{
    providers::ProviderError,
    types::{transaction::eip2718::TypedTransaction, Address, TransactionReceipt, H256},
};
use std::{error::Error, fmt::Debug};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct TransactionId(pub String);

impl AsRef<str> for TransactionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

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

    #[error("Transaction dropped from mempool: {0}.")]
    Dropped(H256),

    #[error("Transaction failed: {0:?}.")]
    Failed(Option<TransactionReceipt>),
}

#[async_trait]
pub trait WriteProvider: Sync + Send + Debug {
    async fn send_transaction(
        &self,
        tx: TypedTransaction,
        only_once: bool,
    ) -> Result<TransactionId, TxError>;

    fn address(&self) -> Address;
}
