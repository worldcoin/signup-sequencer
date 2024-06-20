use std::collections::HashMap;
use std::sync::Arc;

use anyhow::bail;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::Address;
pub use read::ReadProvider;
use tracing::instrument;
pub use write::TxError;

use self::write_provider::WriteProvider;
use crate::config::Config;
use crate::identity::processor::TransactionId;

pub mod read;
pub mod write;

mod write_provider;

#[derive(Clone, Debug)]
pub struct Ethereum {
    read_provider:            Arc<ReadProvider>,
    // Mapping of chain id to provider
    secondary_read_providers: HashMap<u64, Arc<ReadProvider>>,
    write_provider:           Arc<WriteProvider>,
}

impl Ethereum {
    #[instrument(name = "Ethereum::new", level = "debug", skip_all)]
    pub async fn new(config: &Config) -> anyhow::Result<Self> {
        let Some(providers_config) = &config.providers else {
            bail!("Providers config is required for Ethereum.");
        };

        let Some(relayer_config) = &config.relayer else {
            bail!("Relayer config is required for Ethereum.");
        };

        let read_provider =
            ReadProvider::new(providers_config.primary_network_provider.clone().into()).await?;

        let mut secondary_read_providers = HashMap::new();

        for secondary_url in &providers_config.relayed_network_providers.0 {
            let secondary_read_provider = ReadProvider::new(secondary_url.clone().into()).await?;
            secondary_read_providers.insert(
                secondary_read_provider.chain_id.as_u64(),
                Arc::new(secondary_read_provider),
            );
        }

        let write_provider: Arc<WriteProvider> =
            Arc::new(WriteProvider::new(read_provider.clone(), relayer_config).await?);

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
        tracing::info!(?tx, "Sending transaction");
        self.write_provider.send_transaction(tx, only_once).await
    }

    pub async fn fetch_pending_transactions(&self) -> Result<Vec<TransactionId>, TxError> {
        self.write_provider.fetch_pending_transactions().await
    }

    pub async fn mine_transaction(&self, tx: TransactionId) -> Result<bool, TxError> {
        self.write_provider.mine_transaction(tx).await
    }
}
