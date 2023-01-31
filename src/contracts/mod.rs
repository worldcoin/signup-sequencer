//! Functionality for interacting with smart contracts deployed on chain.
pub mod batching;
pub mod confirmed_log_query;
pub mod legacy;

use crate::{
    contracts::legacy::MemberAddedEvent,
    ethereum::{write::TransactionId, Ethereum, EventError, Log, TxError},
};
use async_trait::async_trait;
use clap::Parser;
use ethers::prelude::{Address, U256};
use futures::Stream;
use semaphore::Field;
use std::{pin::Pin, sync::Arc};

/// Configuration options for the component responsible for interacting with the
/// contract.
#[derive(Clone, Debug, PartialEq, Eq, Parser)]
#[group(skip)]
pub struct Options {
    /// The address of the identity manager contract.
    #[clap(long, env, default_value = "174ee9b5fBb5Eb68B6C61032946486dD9c2Dc4b6")]
    pub semaphore_address: Address,

    // TODO This option should be removed.
    /// The semaphore group identifier to use.
    #[clap(long, env, default_value = "1")]
    pub group_id: U256,

    // TODO This option should be removed.
    /// When set, it will create the group if it does not exist with the given
    /// depth.
    #[clap(long, env)]
    pub create_group_depth: Option<usize>,

    /// Initial value of the Merkle tree leaves. Defaults to the initial value
    /// in the identity manager contract.
    #[clap(
        long,
        env,
        default_value = "0000000000000000000000000000000000000000000000000000000000000000"
    )]
    pub initial_leaf_value: Field,
}

/// A trait representing an identity manager that is able to submit user
/// identities to a contract located on the blockchain.
#[async_trait]
pub trait IdentityManager {
    /// Create and configure a new instance of the identity manager.
    ///
    /// # Arguments
    ///
    /// - `options`: The options used to configure the identity manager.
    /// - `ethereum`: A connector for an ethereum-compatible blockchain.
    async fn new(options: Options, ethereum: Ethereum) -> anyhow::Result<Self>
    where
        Self: Sized;

    /// Returns the depth of the merkle tree managed by this `IdentityManager`.
    fn tree_depth(&self) -> usize;

    /// Returns the value used for a newly initialized merkle tree leaf.
    fn initial_leaf_value(&self) -> Field;

    /// Returns the group identifier associated with the identity manager.
    fn group_id(&self) -> U256;

    /// Returns the number of the latest block that is confirmed to have been
    /// mined.
    async fn confirmed_block_number(&self) -> Result<u64, EventError>;

    /// Returns `true` if this `IdentityManager` acts via the manager address of
    /// the on-chain contract it manages.
    async fn is_owner(&self) -> anyhow::Result<bool>;

    /// Registers the provided `identity_commitments` with the contract on
    /// chain.
    async fn register_identities(
        &self,
        identity_commitments: Vec<Field>,
    ) -> Result<TransactionId, TxError>;

    /// Asserts that the provided `root` is the current root held by the
    /// contract on the chain.
    async fn assert_latest_root(&self, root: Field) -> anyhow::Result<()>;

    /// Asserts that the provided `root` is a valid root.
    ///
    /// A valid root is one that has not expired based on the time since it was
    /// inserted into the history of roots on chain.
    async fn assert_valid_root(&self, root: Field) -> anyhow::Result<()>;

    // TODO [Ara] Remove this once the OZ relay work is integrated.
    /// Fetches member added events from the blockchain from a starting block to
    /// an optionally specified end block.
    ///
    /// Such functionality need not be supported by all identity managers, and
    /// these may return `None` to signify such a situation.
    fn fetch_events(&self, starting_block: u64, end_block: Option<u64>) -> Option<EventStream>;
}

/// The type of the event stream used by the contracts to receive events from on
/// chain.
type EventStream<'a> =
    Pin<Box<dyn Stream<Item = Result<Log<MemberAddedEvent>, EventError>> + Send + 'a>>;

/// A type for an identity manager object that can be sent across threads.
pub type SharedIdentityManager = Arc<dyn IdentityManager + Send + Sync>;
