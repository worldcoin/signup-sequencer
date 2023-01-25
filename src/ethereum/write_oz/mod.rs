use std::{sync::Arc, time::Duration};

use self::openzeppelin::OzRelay;
use async_trait::async_trait;
use clap::Parser;
use ethers::types::{transaction::eip2718::TypedTransaction, Address, TransactionReceipt, H160};

use super::{read::duration_from_str, write::WriteProvider, TxError};

mod openzeppelin;

// TODO: Log and metrics for signer / nonces.
#[derive(Clone, Debug, Eq, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    /// OpenZeppelin Defender API Key
    #[clap(long, env)]
    pub oz_api_key: String,

    /// OpenZeppelin Defender API Secret
    #[clap(long, env)]
    pub oz_api_secret: String,

    /// OpenZeppelin Defender API Secret
    #[clap(
        long,
        env,
        default_value = "0x30dcc24131223d4f8af69226e7b11b83e6a68b8b"
    )]
    pub oz_address: H160,

    /// How long pending transactions should be retried
    #[clap(long, env, value_parser=duration_from_str, default_value="60")]
    pub oz_transaction_validity: Duration,
}

#[derive(Clone, Debug)]
pub struct Provider {
    inner:   Arc<OzRelay>,
    address: Address,
}

impl Provider {
    #[allow(dead_code)]
    pub fn new(options: &Options) -> Self {
        let relay = OzRelay::new(&options.oz_api_key, &options.oz_api_secret);

        Self {
            inner:   Arc::new(relay),
            address: options.oz_address,
        }
    }
}

#[async_trait]
impl WriteProvider for Provider {
    async fn send_transaction(
        &self,
        tx: TypedTransaction,
        is_retry: bool,
    ) -> Result<TransactionReceipt, TxError> {
        self.inner.send_transaction(tx, is_retry).await
    }

    fn address(&self) -> Address {
        self.address
    }
}
