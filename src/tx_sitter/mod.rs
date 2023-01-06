use crate::{
    database::{Database, InsertTxError},
    ethereum::{Ethereum, TxError},
};
use ethers::types::{transaction::eip2718::TypedTransaction, TransactionReceipt};
/// This module is meant to enable reliable eth tx transactions. The rest of
/// signup-sequencer can assume that transactions sent through this module will
/// survive crashes and eventually find their way into the blockchain.
///
/// This is a separate module because we may eventually pull it out into a
/// separate crate and then into an independent service. A list of goals and
/// features can be found [here](https://www.notion.so/worldcoin/tx-sitter-8ca70eec826e4491b500f55f03ec1b43).
use std::sync::Arc;

pub struct Sitter {
    database: Arc<Database>,
    ethereum: Ethereum,
}

impl Sitter {
    #[allow(clippy::unused_async)]
    pub async fn new(database: Arc<Database>, ethereum: Ethereum) -> Result<Self, anyhow::Error> {
        Ok(Self { database, ethereum })
    }

    pub async fn send(
        &self,
        id: &[u8],
        tx: TypedTransaction,
    ) -> Result<TransactionReceipt, anyhow::Error> {
        let res = self.database.insert_transaction_request(id, &tx).await;

        // this is ignored because we're not currently using the database, we're
        // immediately sending the transaction to the eth network. Once we start
        // sending transactions from the database we can turn DuplicateTransactionId
        // into a real error.
        //
        // This will require a migration which empties the database of all
        // transaction_requests, because we're not saving any submitted_eth_tx records.

        if let Err(err) = res {
            if !matches!(err, InsertTxError::DuplicateTransactionId) {
                return Err(err.into());
            }
        }

        Ok(self.ethereum.send_transaction(tx).await?)
    }
}
