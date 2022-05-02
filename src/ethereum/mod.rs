mod contract;

use self::contract::{MemberAddedFilter, SemaphoreAirdrop};
use ethers::{
    core::k256::ecdsa::SigningKey,
    middleware::{NonceManagerMiddleware, SignerMiddleware},
    prelude::{H160, U64},
    providers::{Http, Middleware, Provider},
    signers::{LocalWallet, Signer, Wallet},
    types::{Address, H256, U256},
};
use eyre::{eyre, Result as EyreResult};
use semaphore::Field;
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
    pub signing_key: H256,

    /// If this module is being run with EIP-1559 support, useful in some places
    /// where EIP-1559 is not yet supported
    #[structopt(short, parse(try_from_str), default_value = "true", env = "USE_EIP1559")]
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
            let signing_key = SigningKey::from_bytes(options.signing_key.as_bytes())?;
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
    ) -> EyreResult<Vec<(usize, Field)>> {
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
                let mut bytes = [0u8; 32];
                event.identity_commitment.to_big_endian(&mut bytes);
                // TODO: Check for < Modulus.
                let leaf = Field::from_be_bytes_mod_order(&bytes);
                let res = (index, leaf);
                index += 1;
                res
            })
            .collect::<Vec<_>>();
        Ok(insertions)
    }

    pub async fn insert_identity(
        &self,
        group_id: usize,
        commitment: &Field,
        tree_depth: usize,
    ) -> EyreResult<()> {
        info!(%group_id, %commitment, "Inserting identity in contract");
        if self.mock {
            info!(%commitment, "MOCK mode enabled, skipping");
            return Ok(());
        }

        info!(?self.address, "My address");
        let manager = self.semaphore.manager().call().await?;
        info!(?manager, "Fetched manager address");
        if manager != self.address {
            return Err(eyre!("Not the manager"));
        }

        let depth = self
            .semaphore
            .get_depth(group_id.into())
            .from(self.address)
            .call()
            .await?;

        info!(?group_id, ?depth, "Fetched group tree depth");
        if depth == 0 {
            // Must subtract one as internal rust merkle tree is eth merkle tree depth + 1
            let mut tx = self.semaphore.create_group(
                group_id.into(),
                (tree_depth - 1).try_into()?,
                0.into(),
            );
            let create_group_pending_tx = if self.eip1559 {
                self.provider.fill_transaction(&mut tx.tx, None).await?;
                tx.tx.set_gas(10_000_000_u64); // HACK: ethers-rs estimate is wrong.
                info!(?tx, "Sending transaction");
                self.provider.send_transaction(tx.tx, None).await?
            } else {
                // Our tests use ganache which doesn't support EIP-1559 transactions yet.
                tx = tx.legacy();
                info!(?tx, "Sending transaction");
                self.provider.send_transaction(tx.tx, None).await?
            };

            let receipt = create_group_pending_tx
                .await
                .map_err(|e| eyre!(e))?
                .ok_or_else(|| eyre!("tx dropped from mempool"))?;
            if receipt.status != Some(U64::from(1_u64)) {
                return Err(eyre!("tx failed"));
            }
        }
        let commitment = U256::from(commitment.to_be_bytes());
        let mut tx = self.semaphore.add_member(group_id.into(), commitment);
        let pending_tx = if self.eip1559 {
            self.provider.fill_transaction(&mut tx.tx, None).await?;
            tx.tx.set_gas(10_000_000_u64); // HACK: ethers-rs estimate is wrong.
            info!(?tx, "Sending transaction");
            self.provider.send_transaction(tx.tx, None).await?
        } else {
            // Our tests use ganache which doesn't support EIP-1559 transactions yet.
            tx = tx.legacy();
            info!(?tx, "Sending transaction");
            self.provider.send_transaction(tx.tx, None).await?
        };
        let receipt = pending_tx
            .await
            .map_err(|e| eyre!(e))?
            .ok_or_else(|| eyre!("tx dropped from mempool"))?;
        info!(?receipt, "Receipt");
        if receipt.status != Some(U64::from(1_u64)) {
            return Err(eyre!("tx failed"));
        }
        Ok(())
    }
}
