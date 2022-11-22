mod abi;
pub mod caching_log_query;

use self::abi::{MemberAddedFilter, SemaphoreContract as Semaphore};
use crate::{
    database::Database,
    ethereum::{Ethereum, EventError, ProviderStack},
};
use clap::Parser;
use core::future;
use ethers::{
    providers::Middleware,
    types::{Address, U256},
};
use eyre::{eyre, Result as EyreResult};
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
    pub group_id: U256,

    /// When set, it will create the group if it does not exist with the given
    /// depth.
    #[clap(long, env)]
    pub create_group_depth: Option<usize>,

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
    group_id:     U256,
    tree_depth:   usize,
    initial_leaf: Field,
}

impl Contracts {
    #[instrument(level = "debug", skip_all)]
    pub async fn new(options: Options, ethereum: Ethereum) -> EyreResult<Self> {
        // Sanity check the group id
        if options.group_id == U256::zero() {
            error!(group_id = ?options.group_id, "Invalid group id: must be greater than zero");
            return Err(eyre!("group id must be non-zero"));
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
            return Err(eyre!("Invalid Semaphore address"));
        }

        // Connect to Contract
        let semaphore = Semaphore::new(options.semaphore_address, ethereum.provider().clone());

        // Test contract by calling a view function and make sure we are manager.
        let manager = semaphore.manager().call().await?;
        if manager != ethereum.address() {
            error!(?manager, signer = ?ethereum.address(), "Signer is not the manager of the Semaphore contract");
            // return Err(eyre!("Signer is not manager"));
            // TODO: If not manager, proceed in read-only mode.
        }
        info!(?address, ?manager, "Connected to Semaphore contract");

        // Make sure the group exists.
        let tree_depth = semaphore.get_depth(options.group_id).call().await?;
        if tree_depth == 0 {
            if let Some(new_depth) = options.create_group_depth {
                info!(
                    "Group {} not found, creating it with depth {}",
                    options.group_id, new_depth
                );
                let tx = semaphore
                    .create_group(
                        options.group_id,
                        new_depth.try_into()?,
                        options.initial_leaf.to_be_bytes().into(),
                    )
                    .tx;
                ethereum.send_transaction(tx).await?;
            } else {
                error!(group_id = ?options.group_id, "Group does not exist");
                return Err(eyre!("Group does not exist"));
            }
        } else {
            info!(group_id = ?options.group_id, ?tree_depth, "Semaphore group found.");
        }

        // TODO: Some way to check the initial leaf

        Ok(Self {
            ethereum,
            semaphore,
            group_id: options.group_id,
            tree_depth: usize::from(tree_depth),
            initial_leaf: options.initial_leaf,
        })
    }

    #[must_use]
    pub const fn group_id(&self) -> U256 {
        self.group_id
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
    ) -> impl Stream<Item = Result<(usize, Field, Field), EventError>> + '_ {
        info!(
            starting_block,
            last_leaf, "Reading MemberAdded events from chains"
        );
        // TODO: Register to the event stream and track it going forward.

        // Start MemberAdded log event stream
        let filter = self
            .semaphore
            .member_added_filter()
            .from_block(starting_block);
        self.ethereum
            .fetch_events::<MemberAddedEvent>(&filter.filter, database)
            .try_filter(|event| future::ready(event.group_id == self.group_id))
            .enumerate()
            .map(|(index, res)| res.map(|event| (index, event)))
            .map_ok(|(index, event)| {
                (
                    index,
                    // TODO: Validate values < modulus
                    event.identity_commitment.into(),
                    event.root.into(),
                )
            })
    }

    #[instrument(level = "debug", skip_all)]
    #[allow(dead_code)]
    pub async fn is_manager(&self) -> EyreResult<bool> {
        info!(address = ?self.ethereum.address(), "My address");
        let manager = self.semaphore.manager().call().await?;
        info!(?manager, "Fetched manager address");
        Ok(manager == self.ethereum.address())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn insert_identity(&self, commitment: &Field) -> EyreResult<()> {
        info!(%commitment, "Inserting identity in contract");

        // Send create tx
        let commitment = U256::from(commitment.to_be_bytes());
        self.ethereum
            .send_transaction(self.semaphore.add_member(self.group_id, commitment).tx)
            .await?;
        Ok(())
    }

    // TODO: Ideally we'd have a `get_root` function, but the contract doesn't
    // support this.
    #[instrument(level = "debug", skip_all)]
    pub async fn assert_valid_root(&self, root: Field) -> EyreResult<()> {
        // HACK: Abuse the `verifyProof` function.
        let result = self
            .semaphore
            .verify_proof(
                root.to_be_bytes().into(),
                self.group_id,
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
            return Err(eyre!("Invalid root"));
        }
        Err(eyre!("Error verifiying root: {}", result))
    }
}
