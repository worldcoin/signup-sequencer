/// This module is meant to enable reliable eth tx transactions. The rest of
/// signup-sequencer can assume that transactions sent through this module will
/// survive crashes and eventually find their way into the blockchain.
///
/// This is a separate module because we may eventually pull it out into a
/// separate crate and then into an independent service. A list of goals and
/// features can be found [here](https://www.notion.so/worldcoin/tx-sitter-8ca70eec826e4491b500f55f03ec1b43).
use crate::{
    database::{Database, InsertTxError},
    ethereum::{Ethereum, Options, TxError},
};
use ethers::types::{transaction::eip2718::TypedTransaction, TxHash, H256};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub struct Sitter {
    database:        Arc<Database>,
    background_task: tokio::task::JoinHandle<()>,
}

pub enum TxStatus {
    Waiting,
    Submitted,
    Mined {
        block_num:  u64,
        block_hash: H256,
        tx_hash:    TxHash,
    },
    Finalized {
        block_num:  u64,
        block_hash: H256,
        tx_hash:    TxHash,
    },
}

impl Sitter {
    pub async fn new(database: Arc<Database>, options: Options) -> Result<Self, anyhow::Error> {
        let eth = Ethereum::new(options).await?;
        let background_task = tokio::spawn(Self::background_task(database.clone(), eth));
        Ok(Self {
            database,
            background_task,
        })
    }

    async fn background_task(database: Arc<Database>, eth: Ethereum) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            let (txid, tx) = match database.unsubmitted_transaction_request().await {
                Ok(Some((txid, tx))) => (txid, tx),
                Ok(None) => {
                    continue;
                }
                Err(err) => {
                    panic!("failed to get unsubmitted transaction: {}", err);
                }
            };

            let receipt = eth.send_transaction(tx).await
                .expect("TODO: do somthing sensible with this error");

            database.submit_transaction(&txid, &receipt).await
                .expect("TODO: do somthing sensible with this error");
        }
    }

    /// Send a transaction to the blockchain. `id` is used to make this method
    /// safe in the presence of crashes. Once this function returns the
    /// transaction has been committed to the database and will be retried
    /// until it is mined. However, if we crash signup-sequencer may forget
    /// that it has sent this transaction. `id` should be deterministically
    /// derived from the operation we are trying to perform. For example, if
    /// we are submitting a series of batches this might be the batch
    /// number. This method will return [`Error::DuplicateTransactionId`] if
    /// a transaction with this id has already been sent.
    pub async fn send(&self, id: &[u8], tx: TypedTransaction) -> Result<(), InsertTxError> {
        self.database.insert_transaction_reqest(id, tx).await
    }

    pub async fn last_sent_id(&self) -> Result<Option<&[u8]>, Error> {
        unimplemented!()
    }

    pub async fn last_sent_tx(&self) -> Result<Option<TypedTransaction>, Error> {
        unimplemented!()
    }

    pub async fn status(&self, _id: &[u8]) -> Result<TxStatus, Error> {
        unimplemented!()
    }
}
