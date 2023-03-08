use std::sync::Arc;

use anyhow::Result as AnyhowResult;
use clap::Parser;
use ethers::types::{transaction::eip2718::TypedTransaction, Address};
use tracing::instrument;

pub use read::{EventError, ReadProvider};
pub use write::TxError;

use self::write::{TransactionId, WriteProvider};

pub mod read;
pub mod write;
mod write_dev;
mod write_oz;

// TODO: Log and metrics for signer / nonces.
#[derive(Clone, Debug, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    #[clap(flatten)]
    pub read_options: read::Options,

    #[cfg(not(feature = "oz-provider"))]
    #[clap(flatten)]
    pub write_options: write_dev::Options,

    #[cfg(feature = "oz-provider")]
    #[clap(flatten)]
    pub write_options: write_oz::Options,
}

#[derive(Clone, Debug)]
pub struct Ethereum {
    read_provider:  Arc<ReadProvider>,
    write_provider: Arc<dyn WriteProvider>,
}

impl Ethereum {
    #[instrument(name = "Ethereum::new", level = "debug", skip_all)]
    pub async fn new(options: Options) -> AnyhowResult<Self> {
        let read_provider = ReadProvider::new(options.read_options).await?;

        #[cfg(not(feature = "oz-provider"))]
        let write_provider: Arc<dyn WriteProvider> =
            Arc::new(write_dev::Provider::new(read_provider.clone(), options.write_options).await?);

        #[cfg(feature = "oz-provider")]
        let write_provider: Arc<dyn WriteProvider> =
            Arc::new(write_oz::Provider::new(&options.write_options).await?);

        Ok(Self {
            read_provider: Arc::new(read_provider),
            write_provider,
        })
    }

    #[must_use]
    pub const fn provider(&self) -> &Arc<ReadProvider> {
        &self.read_provider
    }

    #[must_use]
    pub fn address(&self) -> Address {
        self.write_provider.address()
    }

    pub async fn send_transaction(
        &self,
        tx: TypedTransaction,
        only_once: bool,
    ) -> Result<TransactionId, TxError> {
        self.write_provider.send_transaction(tx, only_once).await
    }
}
