/// This module is meant to enable reliable eth tx transactions. The rest of
/// signup-sequencer can assume that transactions sent through this module will
/// survive crashes and eventually find their way into the blockchain.
///
/// This is a separate module because we may eventually pull it out into a
/// separate crate and then into an independent service. A list of goals and
/// features can be found [here](https://www.notion.so/worldcoin/tx-sitter-8ca70eec826e4491b500f55f03ec1b43).
use crate::ethereum::{Ethereum, TxError};
use ethers::types::{transaction::eip2718::TypedTransaction, TransactionReceipt};

pub struct Sitter {
    pub ethereum: Ethereum,
}

impl Sitter {
    #[allow(clippy::unused_async)]
    pub async fn new(ethereum: Ethereum) -> Result<Self, anyhow::Error> {
        Ok(Self { ethereum })
    }

    pub async fn send(&self, tx: TypedTransaction) -> Result<TransactionReceipt, TxError> {
        self.ethereum.send_transaction(tx).await
    }
}
