mod contract;

use self::contract::{MemberAddedFilter, SemaphoreAirdrop};
use ethers::{
    core::k256::ecdsa::SigningKey,
    middleware::{NonceManagerMiddleware, SignerMiddleware},
    prelude::H160,
    providers::{Http, Middleware, Provider},
    signers::{LocalWallet, Signer, Wallet},
    types::Address,
};
use eyre::{eyre, Result as EyreResult};
use semaphore::hash::Hash;
use std::sync::Arc;
use structopt::StructOpt;
use tracing::info;
use url::Url;

#[derive(Clone, Debug, PartialEq, StructOpt)]
pub struct Options {
    /// Ethereum API Provider
    #[structopt(long, env, default_value = "http://localhost:8545")]
    pub ethereum_provider: Url,

    /// Semaphore contract address.
    #[structopt(long, env, default_value = "174ee9b5fBb5Eb68B6C61032946486dD9c2Dc4b6")]
    pub semaphore_address: Address,

    /// Private key used for transaction signing
    #[structopt(
        long,
        env,
        default_value = "ee79b5f6e221356af78cf4c36f4f7885a11b67dfcc81c34d80249947330c0f82"
    )]
    // NOTE: We abuse `Hash` here because it has the right `FromStr` implementation.
    pub signing_key: Hash,

    /// If this module is being run with EIP-1559 support, useful in some places
    /// where EIP-1559 is not yet supported
    #[structopt(short, parse(try_from_str), default_value = "true")]
    pub eip1559: bool,

    #[structopt(
        short,
        parse(try_from_str),
        default_value = "false",
        env = "SIGNUP_SEQUENCER_MOCK"
    )]
    pub mock: bool,
}

// Code out the provider stack in types
// Needed because of <https://github.com/gakonst/ethers-rs/issues/592>
type Provider0 = Provider<Http>;
type Provider1 = SignerMiddleware<Provider0, Wallet<SigningKey>>;
type Provider2 = NonceManagerMiddleware<Provider1>;
type ProviderStack = Provider2;

pub struct Ethereum {
    provider:  Arc<ProviderStack>,
    address:   H160,
    semaphore: SemaphoreAirdrop<ProviderStack>,
    eip1559:   bool,
    mock:      bool,
}

impl Ethereum {
    pub async fn new(options: Options) -> EyreResult<Self> {
        // Connect to the Ethereum provider
        // TODO: Support WebSocket and IPC.
        // Blocked on <https://github.com/gakonst/ethers-rs/issues/592>
        let (provider, chain_id) = {
            info!(
                provider = %&options.ethereum_provider,
                "Connecting to Ethereum"
            );
            let http = Http::new(options.ethereum_provider);
            let provider = Provider::new(http);
            let chain_id = provider.get_chainid().await?;
            let latest_block = provider.get_block_number().await?;
            info!(%chain_id, %latest_block, "Connected to Ethereum");
            (provider, chain_id)
        };

        // TODO: Add metrics layer that measures the time each rpc call takes.
        // TODO: Add logging layer that logs calls to major RPC endpoints like
        // send_transaction.

        // Construct a local key signer
        let (provider, address) = {
            let signing_key = SigningKey::from_bytes(options.signing_key.as_bytes_be())?;
            let signer = LocalWallet::from(signing_key);
            let address = signer.address();
            let chain_id: u64 = chain_id.try_into().map_err(|e| eyre!("{}", e))?;
            let signer = signer.with_chain_id(chain_id);
            let provider = SignerMiddleware::new(provider, signer);
            info!(?address, "Constructed wallet");
            (provider, address)
        };

        // TODO: Integrate gas price oracle to not rely on node's `eth_gasPrice`

        // Manage nonces locally
        let provider = { NonceManagerMiddleware::new(provider, address) };

        // Add a 10 block delay to avoid having to handle re-orgs
        // TODO: Pending <https://github.com/gakonst/ethers-rs/pull/568/files>
        // let provider = {
        //     const BLOCK_DELAY: u8 = 10;
        //     TimeLag::<BLOCK_DELAY>::new(provider)
        // };

        // Connect to Contract
        let provider = Arc::new(provider);
        let semaphore = SemaphoreAirdrop::new(options.semaphore_address, provider.clone());
        // TODO: Test contract connection by calling a view function.

        Ok(Self {
            provider,
            address,
            semaphore,
            eip1559: options.eip1559,
            mock: options.mock,
        })
    }

    pub async fn last_block(&self) -> EyreResult<u64> {
        let block_number = self.provider.get_block_number().await?;
        Ok(block_number.as_u64())
    }

    pub async fn fetch_events(
        &self,
        starting_block: u64,
        last_leaf: usize,
    ) -> EyreResult<Vec<(usize, Hash)>> {
        info!(starting_block, "Reading MemberAdded events from chains");
        // TODO: Some form of pagination.
        // TODO: Register to the event stream and track it going forward.
        if self.mock {
            info!(starting_block, "MOCK mode enabled, skipping");
            return Ok(vec![]);
        }
        let filter = self
            .semaphore
            .member_added_filter()
            .from_block(starting_block);
        let events: Vec<MemberAddedFilter> = filter.query().await?;
        info!(count = events.len(), "Read events");
        let mut index = last_leaf;
        let insertions = events
            .iter()
            .map(|event| {
                let res = (index, event.leaf.into());
                index += 1;
                res
            })
            .collect::<Vec<_>>();
        Ok(insertions)
    }

    pub async fn insert_identity(
        &self,
        group_id: usize,
        commitment: &Hash,
        tree_depth: usize,
    ) -> EyreResult<()> {
        info!(%commitment, "Inserting identity in contract");
        if self.mock {
            info!(%commitment, "MOCK mode enabled, skipping");
            return Ok(());
        }
        let depth = self.semaphore.get_depth(group_id.into()).call().await?;
        if depth == 0 {
            // Must subtract one as internal rust merkle tree is eth merkle tree depth + 1
            let create_airdrop_tx = self.semaphore.create_airdrop(
                group_id.into(),
                self.address,
                (tree_depth - 1).try_into()?,
            );
            let create_airdrop_pending_tx = if self.eip1559 {
                self.provider
                    .send_transaction(create_airdrop_tx.tx, None)
                    .await?
            } else {
                // Our tests use ganache which doesn't support EIP-1559 transactions yet.
                self.provider
                    .send_transaction(create_airdrop_tx.legacy().tx, None)
                    .await?
            };
            let create_airdrop_receipt = create_airdrop_pending_tx.await.map_err(|e| eyre!(e))?;
            if create_airdrop_receipt.is_none() {
                // This should only happen if the tx is no longer in the mempool, meaning the tx
                // was dropped.
                return Err(eyre!("tx dropped from mempool"));
            }
        }
        let add_recipient_tx = self
            .semaphore
            .add_recipient(group_id.into(), commitment.into());
        let add_recipient_pending_tx = if self.eip1559 {
            self.provider
                .send_transaction(add_recipient_tx.tx, None)
                .await?
        } else {
            // Our tests use ganache which doesn't support EIP-1559 transactions yet.
            self.provider
                .send_transaction(add_recipient_tx.legacy().tx, None)
                .await?
        };
        let add_recipient_receipt = add_recipient_pending_tx.await.map_err(|e| eyre!(e))?;
        if add_recipient_receipt.is_none() {
            // This should only happen if the tx is no longer in the mempool, meaning the tx
            // was dropped.
            return Err(eyre!("tx dropped from mempool"));
        }
        Ok(())
    }
}
