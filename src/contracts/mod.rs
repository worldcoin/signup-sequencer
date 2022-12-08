mod abi;
pub mod caching_log_query;

use self::abi::{MemberAddedFilter, SemaphoreContract as Semaphore};
use crate::{
    database::Database,
    ethereum::{Ethereum, EventError, ProviderStack, TxError},
};
use anyhow::{anyhow, Result as AnyhowResult};
use clap::Parser;
use ethers::{
    providers::Middleware,
    types::{Address, TransactionReceipt, U256},
};
use futures::{Stream, StreamExt, TryStreamExt};
use semaphore::Field;
use std::sync::Arc;
use tracing::{error, info, instrument};

pub type MemberAddedEvent = MemberAddedFilter;

#[derive(Clone, Debug, PartialEq, Eq, Parser)]
#[group(skip)]
pub struct Options {
    /// Semaphore contract address.
    #[clap(long, env, default_value = "174ee9b5fBb5Eb68B6C61032946486dD9c2Dc4b6")]
    pub semaphore_address: Address,

    /// The Semaphore group id to use
    #[clap(long, env, default_value = "1")]
    pub max_group_id: U256,

    /// When set, it will create the group if it does not exist with the given
    /// depth.
    #[clap(long, env, default_value = "21")]
    pub tree_depth: usize,

    /// Initial value of the Merkle tree leaves. Defaults to the initial value
    /// in Semaphore.sol.
    #[clap(
        long,
        env,
        default_value = "0000000000000000000000000000000000000000000000000000000000000000"
    )]
    pub initial_leaf: Field,
}

pub struct Contracts {
    ethereum:     Ethereum,
    semaphore:    Semaphore<ProviderStack>,
    max_group_id: U256,
    tree_depth:   usize,
    initial_leaf: Field,
}

impl Contracts {
    #[instrument(level = "debug", skip_all)]
    pub async fn new(options: Options, ethereum: Ethereum) -> AnyhowResult<Self> {
        // Sanity check the group id
        if options.max_group_id == U256::zero()  && options.max_group_id < U256::from(256) {
            error!(group_id = ?options.max_group_id, "Invalid max group id: must be greater than zero and smaller than 256");
            return Err(anyhow!("max group id must be greater than zero and smaller than 256"));
        }

        // Sanity check the address
        // TODO: Check that the contract is actually a Semaphore by matching bytecode.
        let address = options.semaphore_address;
        let code = ethereum.provider().get_code(address, None).await?;
        if code.as_ref().is_empty() {
            error!(
                ?address,
                "No contract code deployed at provided Semaphore address"
            );
            return Err(anyhow!("Invalid Semaphore address"));
        }

        // Connect to Contract
        let semaphore = Semaphore::new(options.semaphore_address, ethereum.provider().clone());

        // Test contract by calling a view function and make sure we are manager.
        let manager = semaphore.manager().call().await?;
        if manager != ethereum.address() {
            error!(?manager, signer = ?ethereum.address(), "Signer is not the manager of the Semaphore contract");
            // return Err(anyhow!("Signer is not manager"));
            // TODO: If not manager, proceed in read-only mode.
        }
        info!(?address, ?manager, "Connected to Semaphore contract");

        // Make sure the group exists.
        for group_id in 1..=options.max_group_id.as_usize() {
            let existing_tree_depth = semaphore.get_depth(U256::from(group_id)).call().await?;

            if existing_tree_depth == 0 {
                info!(
                    "Group {} not found, creating it with depth {}",
                    group_id, options.tree_depth
                );
                let tx = semaphore
                    .create_group(
                        group_id.into(),
                        options.tree_depth as u8,
                        options.initial_leaf.to_be_bytes().into(),
                    )
                    .tx;
                ethereum.send_transaction(tx).await?;
            } else if options.tree_depth != usize::from(existing_tree_depth) {
                error!(
                    group_id = group_id,
                    options_depth = options.tree_depth,
                    existing_tree_depth = existing_tree_depth,
                    "Group tree depth does not match"
                );
                return Err(anyhow!("Group tree depth does not match"));
            } else {
                info!(max_group_id = ?options.max_group_id, ?existing_tree_depth, "Semaphore group found.");
            };
        }

        // TODO: Some way to check the initial leaf

        Ok(Self {
            ethereum,
            semaphore,
            max_group_id: options.max_group_id,
            tree_depth: options.tree_depth,
            initial_leaf: options.initial_leaf,
        })
    }

    #[must_use]
    pub const fn max_group_id(&self) -> U256 {
        self.max_group_id
    }

    #[must_use]
    pub const fn tree_depth(&self) -> usize {
        self.tree_depth
    }

    #[must_use]
    pub const fn initial_leaf(&self) -> Field {
        self.initial_leaf
    }

    #[allow(clippy::disallowed_methods)] // False positive from macro expansion.
    #[instrument(level = "debug", skip(self, database))]
    pub fn fetch_events(
        &self,
        starting_block: u64,
        last_leaf: usize,
        database: Arc<Database>,
    ) -> impl Stream<Item = Result<(Field, Field, usize), EventError>> + '_ {
        info!(starting_block, last_leaf, "Reading MemberAdded events");
        // TODO: Register to the event stream and track it going forward.

        // Start MemberAdded log event stream
        let filter = self
            .semaphore
            .member_added_filter()
            .from_block(starting_block);
        self.ethereum
            .fetch_events::<MemberAddedEvent>(&filter.filter, database)
            .map(|res| res)
            .map_ok(|event| {
                (
                    // TODO: Validate values < modulus
                    event.identity_commitment.into(),
                    event.root.into(),
                    event.group_id.as_usize(),
                )
            })
    }

    #[instrument(level = "debug", skip_all)]
    #[allow(dead_code)]
    pub async fn is_manager(&self) -> AnyhowResult<bool> {
        info!(address = ?self.ethereum.address(), "My address");
        let manager = self.semaphore.manager().call().await?;
        info!(?manager, "Fetched manager address");
        Ok(manager == self.ethereum.address())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn insert_identity(
        &self,
        commitment: Field,
        group_id: usize,
    ) -> Result<TransactionReceipt, TxError> {
        info!(%commitment, "Inserting identity in contract");

        // Send create tx
        let commitment = U256::from(commitment.to_be_bytes());
        let receipt = self
            .ethereum
            .send_transaction(self.semaphore.add_member(group_id.into(), commitment).tx)
            .await?;
        Ok(receipt)
    }

    // TODO: Ideally we'd have a `get_root` function, but the contract doesn't
    // support this.
    #[instrument(level = "debug", skip_all)]
    pub async fn assert_valid_root(&self, root: Field, group_id: usize) -> AnyhowResult<()> {
        // HACK: Abuse the `verifyProof` function.

        let result = self
            .semaphore
            .verify_proof(
                root.to_be_bytes().into(),
                group_id.into(),
                U256::zero(),
                U256::zero(),
                U256::zero(),
                [U256::zero(); 8],
            )
            .call()
            .await
            .expect_err("Proof is invalid");
        // Result will be either `0x09bde339`: `InvalidProof()` (good) or
        // `0x504570e3`: `InvalidRoot()` (bad).
        // See <https://github.com/worldcoin/world-id-example-airdrop/blob/03de53d2cb016ddef28b26e8237e85b62ec385c7/src/Semaphore.sol#L141>
        // See <https://sig.eth.samczsun.com/>
        // HACK: There's really no good way to parse these errors
        let error = result.to_string();
        if error.contains("0x09bde339") {
            return Ok(());
        }
        if error.contains("0x504570e3") {
            return Err(anyhow!("Invalid root"));
        }
        Err(anyhow!("Error verifiying root: {}", result))
    }
}
