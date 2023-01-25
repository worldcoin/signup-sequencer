mod read;
mod write_dev;

pub use read::{EventError, Log, ReadProvider};
pub use write_dev::TxError;

use self::write_dev::WriteProvider;
use anyhow::Result as AnyhowResult;
use clap::Parser;
use ethers::types::{transaction::eip2718::TypedTransaction, Address, TransactionReceipt, H160};
use std::{num::ParseIntError, str::FromStr, sync::Arc, time::Duration};
use tracing::instrument;

fn duration_from_str(value: &str) -> Result<Duration, ParseIntError> {
    Ok(Duration::from_secs(u64::from_str(value)?))
}

// TODO: Log and metrics for signer / nonces.
#[derive(Clone, Debug, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    #[clap(flatten)]
    pub read_options: read::Options,

    #[clap(flatten)]
    pub write_options: write_dev::Options,

    /// The number of most recent blocks to be removed from cache on root
    /// mismatch
    #[clap(long, env, default_value = "1000")]
    pub cache_recovery_step_size: usize,

    /// Frequency of event fetching from Ethereum (seconds)
    #[clap(long, env, value_parser=duration_from_str, default_value="60")]
    pub refresh_rate: Duration,
}

#[derive(Clone, Debug)]
pub struct Ethereum {
    read_provider:  Arc<ReadProvider>,
    write_provider: WriteProvider,
    address:        H160,
}

impl Ethereum {
    #[instrument(name = "Ethereum::new", level = "debug", skip_all)]
    pub async fn new(options: Options) -> AnyhowResult<Self> {
        let read_provider = ReadProvider::new(options.read_options).await?;
        let write_provider =
            WriteProvider::new(read_provider.clone(), options.write_options).await?;

        let address = write_provider.address;

        Ok(Self {
            read_provider: Arc::new(read_provider),
            write_provider,
            address,
        })
    }

    #[must_use]
    pub const fn provider(&self) -> &Arc<ReadProvider> {
        &self.read_provider
    }

    #[must_use]
    pub const fn address(&self) -> Address {
        self.address
    }

    pub async fn send_transaction(
        &self,
        tx: TypedTransaction,
    ) -> Result<TransactionReceipt, TxError> {
        self.write_provider.send_transaction(tx).await
    }
}
