mod contract;

use self::contract::{LeafInsertionFilter, Semaphore};
use crate::hash::Hash;
use ethers::{
    core::k256::ecdsa::SigningKey,
    prelude::{Address, Http, LocalWallet, Middleware, Provider, Signer, SignerMiddleware, Wallet},
};
use eyre::{eyre, Result as EyreResult};
use std::sync::Arc;
use structopt::StructOpt;
use tracing::info;
use url::Url;

pub type ContractSigner = SignerMiddleware<Provider<Http>, Wallet<SigningKey>>;

#[derive(Debug, PartialEq, StructOpt)]
pub struct Options {
    /// Ethereum API Provider
    #[structopt(long, env, default_value = "http://localhost:8545")]
    pub ethereum_provider: Url,

    /// Semaphore contract address.
    #[structopt(long, env, default_value = "266FB396B626621898C87a92efFBA109dE4685F6")]
    pub semaphore_address: Address,

    /// Private key used for transaction signing
    #[structopt(
        long,
        env,
        default_value = "ee79b5f6e221356af78cf4c36f4f7885a11b67dfcc81c34d80249947330c0f82"
    )]
    // NOTE: We abuse `Hash` here because it has the right `FromStr` implementation.
    pub signing_key: Hash,
}

pub struct Ethereum {
    provider:  Provider<Http>,
    client:    Arc<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>,
    semaphore: Semaphore<ContractSigner>,
}

impl Ethereum {
    pub async fn new(options: Options) -> EyreResult<Self> {
        // Connect to the Ethereum provider
        // TODO: Support WebSocket and Https
        info!(
            provider = %&options.ethereum_provider,
            "Connecting to Ethereum"
        );
        let http = Http::new(options.ethereum_provider);
        let provider = Provider::new(http);
        let chain_id = provider.get_chainid().await?;
        let latest_block = provider.get_block_number().await?;
        info!(%chain_id, %latest_block, "Connected to Ethereum");

        // Construct wallet
        let chain_id: u64 = chain_id.try_into().map_err(|e| eyre!("{}", e))?;
        let signing_key = SigningKey::from_bytes(options.signing_key.as_bytes_be())?;
        let wallet = LocalWallet::from(signing_key).with_chain_id(chain_id);
        let address = wallet.address();
        info!(?address, "Constructed wallet");

        // Construct middleware stack
        // TODO: See <https://docs.rs/ethers-middleware/0.5.4/ethers_middleware/index.html> for useful middlewares.
        let client = SignerMiddleware::new(provider.clone(), wallet);

        // Connect to Contract
        let client = Arc::new(client);
        let semaphore = Semaphore::new(options.semaphore_address, client.clone());

        Ok(Self {
            provider,
            client,
            semaphore,
        })
    }

    pub async fn last_block(&self) -> EyreResult<u64> {
        let block_number = self.provider.get_block_number().await?;
        Ok(block_number.as_u64())
    }

    pub async fn fetch_events(&self, starting_block: u64) -> EyreResult<Vec<(usize, Hash)>> {
        info!(starting_block, "Reading LeafInsertion events from chains");
        // TODO: Some form of pagination.
        // TODO: Register to the event stream and track it going forward.
        let filter = self
            .semaphore
            .leaf_insertion_filter()
            .from_block(starting_block);
        let events: Vec<LeafInsertionFilter> = filter.query().await?;
        info!(count = events.len(), "Read events");
        let insertions = events
            .iter()
            .map(|event| (event.leaf_index.as_usize(), event.leaf.into()))
            .collect::<Vec<_>>();
        Ok(insertions)
    }

    pub async fn insert_identity(&self, commitment: &Hash) -> EyreResult<()> {
        info!(%commitment, "Inserting identity in contract");
        let tx = self.semaphore.insert_identity(commitment.into());
        let pending_tx = self.client.send_transaction(tx.tx, None).await.unwrap();
        let _receipt = pending_tx.await.map_err(|e| eyre!(e))?;
        // TODO: What does it mean if `_receipt` is None?
        Ok(())
    }
}
