use crate::{
    contracts::{self, Contracts},
    ethereum::{self, Ethereum},
    server::Error as ServerError,
};
use cli_batteries::await_shutdown;
use core::cmp::max;
use ethers::{providers::Middleware, types::U256};
use eyre::{eyre, Result as EyreResult};
use futures::{pin_mut, StreamExt, TryStreamExt};
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
use tokio::{
    select,
    sync::{RwLock, RwLockReadGuard},
};
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

#[derive(Clone, Debug, PartialEq, StructOpt)]
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
    last_block:   u64,
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
        let merkle_tree = PoseidonTree::new(contracts.tree_depth() + 1, contracts.initial_leaf());

        // Wipe storage to force sync from chain
        if options.wipe_storage && options.storage_file.is_file() {
            remove_file(&options.storage_file)?;
        }

        let mut result = Self {
            ethereum,
            contracts,
            storage_file: options.storage_file,
            merkle_tree: RwLock::new(merkle_tree),
            next_leaf: AtomicUsize::new(0),
            last_block: options.starting_block,
        };

        result.read_file().await?;
        result.check_leaves().await?;
        result.process_events().await?;
        // TODO: Store file after processing events.
        result.check_health().await?;
        Ok(result)
    }

    /// Inserts a new commitment into the Merkle tree. This will also update the
    /// contract's commitment tree.
    ///
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
        if commitment == &self.contracts.initial_leaf() {
            warn!(?commitment, next = %self.next_leaf.load(Ordering::Acquire), "Attempt to insert initial leaf.");
            return Err(ServerError::InvalidCommitment);
        }

        // Get a lock on the tree for the duration of this operation.
        // OPT: Sequence operations and allow concurrent inserts / transactions.
        let mut tree = self.merkle_tree.write().await;

        if let Some(existing) = tree.leaves().iter().position(|&x| x == *commitment) {
            warn!(?existing, ?commitment, next = %self.next_leaf.load(Ordering::Acquire), "Commitment already exists in tree.");
            return Err(ServerError::DuplicateCommitment);
        };

        // Fetch next leaf index
        let identity_index = self.next_leaf.fetch_add(1, Ordering::AcqRel);

        // Send Semaphore transaction
        self.contracts
            .insert_identity(commitment)
            .await
            .map_err(|e| {
                error!(?e, "Failed to insert identity to contract.");
                panic!("Failed to submit transaction, state synchronization lost.");
                #[allow(unreachable_code)]
                e
            })?;

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
        commitment: &Hash,
    ) -> Result<InclusionProofResponse, ServerError> {
        if U256::from(group_id) != self.contracts.group_id() {
            return Err(ServerError::InvalidGroupId);
        }
        if commitment == &self.contracts.initial_leaf() {
            return Err(ServerError::InvalidCommitment);
        }

        let merkle_tree = self.merkle_tree.read().await;
        let identity_index = match merkle_tree.leaves().iter().position(|&x| x == *commitment) {
            Some(i) => i,
            None => return Err(ServerError::IdentityCommitmentNotFound),
        };

        let proof = merkle_tree
            .proof(identity_index)
            .ok_or(ServerError::IndexOutOfBounds)?;
        let root = merkle_tree.root();

        // Locally check the proof
        // TODO: Check the leaf index / path
        if !merkle_tree.verify(*commitment, &proof) {
            error!(
                ?commitment,
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
            return Err(ServerError::RootMismatch);
        }

        Ok(InclusionProofResponse { root, proof })
    }

    /// Stores the Merkle tree to the storage file.
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

    #[instrument(level = "info", skip_all)]
    async fn read_file(&mut self) -> EyreResult<()> {
        let mut merkle_tree = self.merkle_tree.write().await;
        if self.storage_file.is_file() {
            let file = File::open(&self.storage_file)?;
            if file.metadata()?.len() > 0 {
                let file: JsonCommitment = serde_json::from_reader(BufReader::new(file))?;
                self.next_leaf
                    .store(file.commitments.len(), Ordering::Release);
                merkle_tree.set_range(0, file.commitments);
                self.last_block = file.last_block;
                info!(path = ?&self.storage_file, num_leaves = %self.next_leaf.load(Ordering::Acquire), last_block = %self.last_block, "Read tree from storage");
            } else {
                warn!(path = ?&self.storage_file, "Storage file empty, skipping.");
            }
        } else {
            warn!(path = ?&self.storage_file, "Storage file not found, skipping.");
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn check_leaves(&self) -> EyreResult<()> {
        let merkle_tree = self.merkle_tree.read().await;
        let next_leaf = self.next_leaf.load(Ordering::Acquire);
        let initial_leaf = self.contracts.initial_leaf();
        for (index, &leaf) in merkle_tree.leaves().iter().enumerate() {
            if index < next_leaf && leaf == initial_leaf {
                error!(
                    ?index,
                    ?leaf,
                    ?next_leaf,
                    "Leaf in non-empty spot set to initial leaf value."
                );
            }
            if index >= next_leaf && leaf != initial_leaf {
                error!(
                    ?index,
                    ?leaf,
                    ?next_leaf,
                    "Leaf in empty spot not set to initial leaf value."
                );
            }
            if leaf != initial_leaf {
                if let Some(previous) = merkle_tree.leaves()[..index]
                    .iter()
                    .position(|&l| l == leaf)
                {
                    error!(?index, ?leaf, ?previous, "Leaf not unique.");
                }
            }
        }
        Ok(())
    }

    #[instrument(level = "info", skip_all)]
    async fn process_events(&mut self) -> EyreResult<()> {
        let mut merkle_tree = self.merkle_tree.write().await;
        let initial_leaf = self.contracts.initial_leaf();
        let mut events = self
            .contracts
            .fetch_events(self.last_block, self.next_leaf.load(Ordering::Acquire))
            .boxed();
        let shutdown = await_shutdown();
        pin_mut!(shutdown);
        loop {
            let (index, leaf, root) = select! {
                v = events.try_next() => match v? {
                    Some(a) => a,
                    None => break,
                },
                _ = &mut shutdown => return Err(eyre!("Interrupted")),
            };
            debug!(?index, ?leaf, ?root, "Received event");

            // Check leaf index is valid
            if index >= merkle_tree.num_leaves() {
                error!(?index, ?leaf, num_leaves = ?merkle_tree.num_leaves(), "Received event out of range");
                panic!("Received event out of range");
            }

            // Check if leaf value is valid
            if leaf == initial_leaf {
                error!(?index, ?leaf, "Inserting empty leaf");
                continue;
            }

            // Check leaf value with existing value
            let existing = merkle_tree.leaves()[index];
            if existing != initial_leaf {
                if existing == leaf {
                    error!(?index, ?leaf, "Received event for already existing leaf.");
                    continue;
                }
                error!(
                    ?index,
                    ?leaf,
                    ?existing,
                    "Received event for already set leaf."
                );
            }

            // Check insertion counter
            if index != self.next_leaf.load(Ordering::Acquire) {
                error!(
                    ?index,
                    ?self.next_leaf,
                    ?leaf,
                    "Event leaf index does not match expected leaf index."
                );
            }

            // Check duplicates
            if let Some(previous) = merkle_tree.leaves()[..index]
                .iter()
                .position(|&l| l == leaf)
            {
                error!(
                    ?index,
                    ?leaf,
                    ?previous,
                    "Received event for already inserted leaf."
                );
            }

            // Insert
            merkle_tree.set(index, leaf);
            self.next_leaf.store(
                max(self.next_leaf.load(Ordering::Acquire), index + 1),
                Ordering::Release,
            );

            // Check root
            if root != merkle_tree.root() {
                error!(computed_root = ?merkle_tree.root(), event_root = ?root, "Root mismatch between event and computed tree.");
                panic!("Root mismatch between event and computed tree.");
            }
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn check_health(&self) -> EyreResult<()> {
        let merkle_tree = self.merkle_tree.read().await;
        let initial_leaf = self.contracts.initial_leaf();
        // TODO: A re-org undoing events would cause this to fail.
        if self.next_leaf.load(Ordering::Acquire) > 0 {
            if let Err(error) = self.contracts.assert_valid_root(merkle_tree.root()).await {
                error!(root = ?merkle_tree.root(), %error, "Root not valid on-chain.");
            } else {
                info!(root = ?merkle_tree.root(), "Root matches on-chain root.");
            }
        } else {
            // TODO: This should still be checkable.
            info!(root = ?merkle_tree.root(), "Empty tree, not checking root.");
        }

        // Check tree health
        let next_leaf = merkle_tree
            .leaves()
            .iter()
            .rposition(|&l| l != initial_leaf)
            .map_or(0, |i| i + 1);
        let used_leaves = &merkle_tree.leaves()[..next_leaf];
        let skipped = used_leaves.iter().filter(|&&l| l == initial_leaf).count();
        let mut dedup = used_leaves
            .iter()
            .filter(|&&l| l != initial_leaf)
            .collect::<Vec<_>>();
        dedup.sort();
        dedup.dedup();
        let unique = dedup.len();
        let duplicates = used_leaves.len() - skipped - unique;
        let total = merkle_tree.num_leaves();
        let available = total - next_leaf;
        #[allow(clippy::cast_precision_loss)]
        let fill = (next_leaf as f64) / (total as f64);
        if skipped == 0 && duplicates == 0 {
            info!(
                healthy = %unique,
                %available,
                %total,
                %fill,
                "Merkle tree is healthy, no duplicates or skipped leaves."
            );
        } else {
            error!(
                healthy = %unique,
                %duplicates,
                %skipped,
                used = %next_leaf,
                %available,
                %total,
                %fill,
                "Merkle tree has duplicate or skipped leaves."
            );
        }
        if next_leaf > available * 3 {
            if next_leaf > available * 19 {
                error!(
                    used = %next_leaf,
                    available = %available,
                    total = %total,
                    "Merkle tree is over 95% full."
                );
            } else {
                warn!(
                    used = %next_leaf,
                    available = %available,
                    total = %total,
                    "Merkle tree is over 75% full."
                );
            }
        }
        Ok(())
    }
}
