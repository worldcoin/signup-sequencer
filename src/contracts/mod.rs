mod abi;

use self::abi::{MemberAddedFilter, Semaphore};
use crate::ethereum::{Ethereum, ProviderStack};
use ethers::{
    abi::RawLog,
    contract::EthEvent,
    providers::Middleware,
    types::{Address, U256, U64},
};
use eyre::{eyre, Result as EyreResult};
use semaphore::Field;
use structopt::StructOpt;
use tracing::{error, info, instrument};

pub type MemberAddedEvent = MemberAddedFilter;

#[derive(Clone, Debug, PartialEq, StructOpt)]
pub struct Options {
    /// Semaphore contract address.
    #[structopt(long, env, default_value = "174ee9b5fBb5Eb68B6C61032946486dD9c2Dc4b6")]
    pub semaphore_address: Address,

    /// The Semaphore group id to use
    #[structopt(long, env, default_value = "1")]
    pub group_id: U256,

    /// Mock mode: do not actually submit transactions.
    #[structopt(
        short,
        parse(try_from_str),
        default_value = "false",
        env = "SIGNUP_SEQUENCER_MOCK"
    )]
    pub mock: bool,
}

pub struct Contracts {
    ethereum:   Ethereum,
    semaphore:  Semaphore<ProviderStack>,
    group_id:   U256,
    tree_depth: usize,
    mock:       bool,
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
            return Err(eyre!("Signer is not manager"));
        }
        info!(?address, ?manager, "Connected to Semaphore contract");

        // Make sure the group exists.
        let tree_depth = semaphore.get_depth(options.group_id).call().await?;
        if tree_depth == 0 {
            error!(group_id = ?options.group_id, "Group does not exist");
            return Err(eyre!("Group does not exist"));
        }
        info!(?tree_depth, "Semaphore group found.");

        // TODO: Create group if not exists and flag is set.

        Ok(Self {
            ethereum,
            semaphore,
            group_id: options.group_id,
            tree_depth: usize::from(tree_depth),
            mock: options.mock,
        })
    }

    pub fn group_id(&self) -> U256 {
        self.group_id
    }

    pub fn tree_depth(&self) -> usize {
        self.tree_depth
    }

    // TODO: Remove this function
    #[instrument(level = "debug", skip_all)]
    pub async fn last_block(&self) -> EyreResult<u64> {
        let block_number = self.ethereum.provider().get_block_number().await?;
        Ok(block_number.as_u64())
    }

    // TODO: Remove this function
    #[instrument(level = "debug", skip_all)]
    pub async fn get_nonce(&self) -> EyreResult<usize> {
        let nonce = self
            .ethereum
            .provider()
            .get_transaction_count(self.ethereum.address(), None)
            .await?;
        Ok(nonce.as_usize())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn fetch_events(
        &self,
        starting_block: u64,
        last_leaf: usize,
    ) -> EyreResult<Vec<(usize, Field, Field)>> {
        info!(starting_block, "Reading MemberAdded events from chains");
        // TODO: Register to the event stream and track it going forward.

        // TODO: Filter by group id.

        // Fetch MemberAdded log events
        let filter = self
            .semaphore
            .member_added_filter()
            .from_block(starting_block);
        let events = self
            .ethereum
            .fetch_events::<MemberAddedEvent>(&filter.filter)
            .await?;

        info!(count = events.len(), "Read events");
        let mut index = last_leaf;
        let insertions = events
            .iter()
            .filter(|event| event.group_id == self.group_id)
            .map(|event| {
                let mut id_bytes = [0u8; 32];
                event.identity_commitment.to_big_endian(&mut id_bytes);

                let mut root_bytes = [0u8; 32];
                event.root.to_big_endian(&mut root_bytes);

                // TODO: Check for < Modulus.
                let root = Field::from_be_bytes_mod_order(&root_bytes);
                let leaf = Field::from_be_bytes_mod_order(&id_bytes);
                let res = (index, leaf, root);
                index += 1;
                res
            })
            .collect::<Vec<_>>();
        Ok(insertions)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn is_manager(&self) -> EyreResult<bool> {
        info!(address = ?self.ethereum.address(), "My address");
        let manager = self.semaphore.manager().call().await?;
        info!(?manager, "Fetched manager address");
        Ok(manager == self.ethereum.address())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn create_group(&self, group_id: usize, tree_depth: usize) -> EyreResult<()> {
        // Must subtract one as internal rust merkle tree is eth merkle tree depth + 1
        let depth = tree_depth - 1;
        self.ethereum
            .send_transaction(
                self.semaphore
                    .create_group(group_id.into(), depth.try_into()?, 0.into())
                    .tx,
            )
            .await?;
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn insert_identity(&self, commitment: &Field, nonce: usize) -> EyreResult<()> {
        info!(%commitment, "Inserting identity in contract");
        if self.mock {
            info!(%commitment, "MOCK mode enabled, skipping");
            return Ok(());
        }

        // Send create tx
        let commitment = U256::from(commitment.to_be_bytes());
        self.ethereum
            .send_transaction(self.semaphore.add_member(self.group_id, commitment).tx)
            .await?;
        Ok(())
    }
}
