use crate::{
    contracts::{self, Contracts},
    ethereum::{self, Ethereum},
    server::Error as ServerError,
};
use core::cmp::max;
use ethers::{providers::Middleware, types::U256};
use eyre::Result as EyreResult;
use futures::{StreamExt, TryStreamExt};
use semaphore::{
    merkle_tree::Hasher,
    poseidon_tree::{PoseidonHash, PoseidonTree, Proof},
    Field,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{remove_file, File},
    io::{BufReader, BufWriter},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use structopt::StructOpt;
use tokio::sync::{RwLock, RwLockReadGuard};
use tracing::{debug, error, info, instrument, warn};

pub type Hash = <PoseidonHash as Hasher>::Hash;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonCommitment {
    pub last_block:  u64,
    pub commitments: Vec<Hash>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexResponse {
    identity_index: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InclusionProofResponse {
    pub root:  Field,
    pub proof: Proof,
}

#[derive(Clone, Debug, PartialEq, Eq, StructOpt)]
pub struct Options {
    #[structopt(flatten)]
    pub ethereum: ethereum::Options,

    #[structopt(flatten)]
    pub contracts: contracts::Options,

    /// Storage location for the Merkle tree.
    #[structopt(long, env, default_value = "commitments.json")]
    pub storage_file: PathBuf,

    /// Wipe database on startup
    #[structopt(long, env, parse(try_from_str), default_value = "false")]
    pub wipe_storage: bool,

    /// Block number to start syncing from
    #[structopt(long, env, default_value = "0")]
    pub starting_block: u64,
}

pub struct App {
    ethereum:     Ethereum,
    contracts:    Contracts,
    storage_file: PathBuf,
    merkle_tree:  RwLock<PoseidonTree>,
    next_leaf:    AtomicUsize,
}

impl App {
    /// # Errors
    ///
    /// Will return `Err` if the internal Ethereum handler errors or if the
    /// `options.storage_file` is not accessible.
    #[allow(clippy::missing_panics_doc)] // TODO
    #[instrument(level = "debug", skip_all)]
    pub async fn new(options: Options) -> EyreResult<Self> {
        let ethereum = Ethereum::new(options.ethereum).await?;
        let contracts = Contracts::new(options.contracts, ethereum.clone()).await?;

        // Poseidon tree depth is one more than the contract's tree depth
        let mut merkle_tree =
            PoseidonTree::new(contracts.tree_depth() + 1, contracts.initial_leaf());
        let mut num_leaves = 0;

        // Wipe storage to force sync from chain
        if options.wipe_storage && options.storage_file.is_file() {
            remove_file(&options.storage_file)?;
        }

        // Read tree from file
        info!(path = ?&options.storage_file, "Reading tree from storage");
        let (mut next_leaf, last_block) = if options.storage_file.is_file() {
            let file = File::open(&options.storage_file)?;
            if file.metadata()?.len() > 0 {
                let file: JsonCommitment = serde_json::from_reader(BufReader::new(file))?;
                let next_leaf = file.commitments.len();
                num_leaves = file.commitments.len();
                merkle_tree.set_range(0, file.commitments);
                (next_leaf, file.last_block)
            } else {
                warn!(path = ?&options.storage_file, "Storage file empty, skipping.");
                (0, options.starting_block)
            }
        } else {
            warn!(path = ?&options.storage_file, "Storage file not found, skipping.");
            (0, options.starting_block)
        };

        // Read events from blockchain
        // TODO: Allow for shutdowns. Write trait to make it easy to add shutdowns (and
        // timeouts?) to futures.
        let mut events = contracts.fetch_events(last_block, num_leaves).boxed();
        while let Some((leaf, hash, root)) = events.try_next().await? {
            debug!(?leaf, ?hash, ?root, "Received event");

            debug!(root = ?merkle_tree.root(), "Prior root");

            // Check leaf index is valid
            if leaf >= merkle_tree.num_leaves() {
                error!(?leaf, num_leaves = ?merkle_tree.num_leaves(), "Received event out of range");
                panic!("Received event out of range");
            }

            // Check if leaf value is valid
            if hash == contracts.initial_leaf() {
                warn!(?leaf, "Trying to add empty leaf, skipping.");
            }

            // Check leaf value with existing value
            let existing = merkle_tree.leaves()[leaf];
            if existing != contracts.initial_leaf() {
                if existing == hash {
                    warn!(
                        ?leaf,
                        ?existing,
                        "Leaf was already correctly set, skipping."
                    );
                    continue;
                }
                error!(
                    ?leaf,
                    ?existing,
                    ?hash,
                    "Event hash contradicts existing leaf."
                );
                panic!("Event hash contradicts existing leaf.");
            }

            // Check insertion counter
            if leaf != next_leaf {
                error!(
                    ?leaf,
                    ?next_leaf,
                    ?hash,
                    "Event leaf index does not match expected leaf."
                );
                panic!("Event leaf does not match expected leaf.");
            }

            // Insert
            merkle_tree.set(leaf, hash);
            next_leaf = max(next_leaf, leaf + 1);

            // Check root
            if root != merkle_tree.root() {
                error!(computed_root = ?merkle_tree.root(), event_root = ?root, "Root mismatch between event and computed tree.");
                panic!("Root mismatch between event and computed tree.");
            }
        }
        drop(events);

        // TODO: Final root check

        Ok(Self {
            ethereum,
            contracts,
            storage_file: options.storage_file,
            merkle_tree: RwLock::new(merkle_tree),
            next_leaf: AtomicUsize::new(next_leaf),
        })
    }

    /// # Errors
    ///
    /// Will return `Err` if the Eth handler cannot insert the identity to the
    /// contract, or if writing to the storage file fails.
    #[instrument(level = "debug", skip_all)]
    pub async fn insert_identity(
        &self,
        group_id: usize,
        commitment: &Hash,
    ) -> Result<IndexResponse, ServerError> {
        if U256::from(group_id) != self.contracts.group_id() {
            return Err(ServerError::InvalidGroupId);
        }

        // Get a lock on the tree for the duration of this operation.
        // OPT: Sequence operations and allow concurrent inserts / transactions.
        let mut tree = self.merkle_tree.write().await;

        // Fetch next leaf index
        let identity_index = self.next_leaf.fetch_add(1, Ordering::AcqRel);

        // Send Semaphore transaction
        self.contracts.insert_identity(commitment).await?;

        // Update and write merkle tree
        tree.set(identity_index, *commitment);

        // Downgrade write lock to read lock
        let tree = tree.downgrade();

        // Check tree root
        if let Err(error) = self.contracts.assert_valid_root(tree.root()).await {
            error!(
                computed_root = ?tree.root(),
                ?error,
                "Root mismatch between tree and contract."
            );
            panic!("Root mismatch between tree and contract.");
        }

        // Immediately write the tree to storage, before anyone else can write.
        self.store(tree).await?;

        Ok(IndexResponse { identity_index })
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided index is out of bounds.
    #[instrument(level = "debug", skip_all)]
    pub async fn inclusion_proof(
        &self,
        group_id: usize,
        identity_commitment: &Hash,
    ) -> Result<InclusionProofResponse, ServerError> {
        if U256::from(group_id) != self.contracts.group_id() {
            return Err(ServerError::InvalidGroupId);
        }

        let merkle_tree = self.merkle_tree.read().await;
        let identity_index = match merkle_tree
            .leaves()
            .iter()
            .position(|&x| x == *identity_commitment)
        {
            Some(i) => i,
            None => return Err(ServerError::IdentityCommitmentNotFound),
        };

        let proof = merkle_tree
            .proof(identity_index)
            .ok_or(ServerError::IndexOutOfBounds)?;
        let root = merkle_tree.root();

        // Locally check the proof
        // TODO: Check the leaf index / path
        if !merkle_tree.verify(*identity_commitment, &proof) {
            error!(
                ?identity_commitment,
                ?identity_index,
                ?root,
                "Proof does not verify locally."
            );
            panic!("Proof does not verify locally.");
        }

        // Verify the root on chain
        if let Err(error) = self.contracts.assert_valid_root(root).await {
            error!(
                computed_root = ?root,
                ?error,
                "Root mismatch between tree and contract."
            );
            panic!("Root mismatch between tree and contract.");
        }

        Ok(InclusionProofResponse { root, proof })
    }

    #[instrument(level = "debug", skip_all)]
    async fn store(&self, tree: RwLockReadGuard<'_, PoseidonTree>) -> EyreResult<()> {
        let file = File::create(&self.storage_file)?;

        // TODO: What we really want here is the last block we processed events from.
        // Also, we need to keep some re-org depth into account (which should be covered
        // by the events already).
        let last_block = self.ethereum.provider().get_block_number().await?.as_u64();
        let next_leaf = self.next_leaf.load(Ordering::Acquire);
        let commitments = tree.leaves()[..next_leaf].to_vec();
        let data = JsonCommitment {
            last_block,
            commitments,
        };
        serde_json::to_writer(BufWriter::new(file), &data)?;
        Ok(())
    }
}
