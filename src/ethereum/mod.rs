use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result as AnyhowResult;
use clap::Parser;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::Address;
pub use read::{EventError, ReadProvider};
use tracing::instrument;
use url::Url;
pub use write::TxError;

use self::write::{TransactionId, WriteProvider};
use crate::serde_utils::JsonStrWrapper;

pub mod read;
pub mod write;

mod write_oz;

// TODO: Log and metrics for signer / nonces.
#[derive(Clone, Debug, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    /// Ethereum API Provider
    #[clap(long, env, default_value = "http://localhost:8545")]
    pub ethereum_provider: Url,

    /// Provider urls for the secondary chains
    #[clap(long, env, default_value = "[]")]
    pub secondary_providers: JsonStrWrapper<Vec<Url>>,

    #[clap(flatten)]
    pub write_options: write_oz::Options,
}

#[derive(Clone, Debug)]
pub struct Ethereum {
    read_provider:            Arc<ReadProvider>,
    // Mapping of chain id to provider
    secondary_read_providers: HashMap<u64, Arc<ReadProvider>>,
    write_provider:           Arc<dyn WriteProvider>,
}

impl Ethereum {
    #[instrument(name = "Ethereum::new", level = "debug", skip_all)]
    pub async fn new(options: Options) -> AnyhowResult<Self> {
        let read_provider = ReadProvider::new(options.ethereum_provider).await?;

        let mut secondary_read_providers = HashMap::new();

        for secondary_url in &options.secondary_providers.0 {
            let secondary_read_provider = ReadProvider::new(secondary_url.clone()).await?;
            secondary_read_providers.insert(
                secondary_read_provider.chain_id.as_u64(),
                Arc::new(secondary_read_provider),
            );
        }

        let write_provider: Arc<dyn WriteProvider> =
            Arc::new(write_oz::Provider::new(read_provider.clone(), &options.write_options).await?);

        Ok(Self {
            read_provider: Arc::new(read_provider),
            secondary_read_providers,
            write_provider,
        })
    }

    #[must_use]
    pub const fn provider(&self) -> &Arc<ReadProvider> {
        &self.read_provider
    }

    #[must_use]
    pub const fn secondary_providers(&self) -> &HashMap<u64, Arc<ReadProvider>> {
        &self.secondary_read_providers
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

    pub async fn fetch_pending_transactions(&self) -> Result<Vec<TransactionId>, TxError> {
        self.write_provider.fetch_pending_transactions().await
    }

    pub async fn mine_transaction(&self, tx: TransactionId) -> Result<bool, TxError> {
        self.write_provider.mine_transaction(tx).await
    }
}
