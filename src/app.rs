use crate::{
    ethereum::{self, Ethereum},
    server::Error as ServerError,
};
use core::cmp::max;
use eyre::Result as EyreResult;
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
use tokio::sync::RwLock;
use tracing::{info, instrument, warn};

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

    /// Storage location for the Merkle tree.
    #[structopt(long, env, default_value = "commitments.json")]
    pub storage_file: PathBuf,

    /// Wipe database on startup
    #[structopt(long, env, parse(try_from_str), default_value = "false")]
    pub wipe_storage: bool,

    /// Number of layers in the tree. Defaults to 21 to match Semaphore.sol
    /// defaults.
    #[structopt(long, env, default_value = "21")]
    pub tree_depth: usize,

    /// Initial value of the Merkle tree leaves. Defaults to the initial value
    /// in Semaphore.sol.
    #[structopt(
        long,
        env,
        default_value = "0000000000000000000000000000000000000000000000000000000000000000"
    )]
    pub initial_leaf: Hash,
}

pub struct App {
    ethereum:     Ethereum,
    storage_file: PathBuf,
    merkle_tree:  RwLock<PoseidonTree>,
    next_leaf:    AtomicUsize,
    tree_depth:   usize,
    is_manager:   bool,
    nonce_offset: usize,
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
        let mut merkle_tree = PoseidonTree::new(options.tree_depth, options.initial_leaf);
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
                (0, 0)
            }
        } else {
            warn!(path = ?&options.storage_file, "Storage file not found, skipping.");
            (0, 0)
        };

        // Read events from blockchain
        let events = ethereum.fetch_events(last_block, num_leaves).await?;
        for (leaf, hash, root) in events {
            merkle_tree.set(leaf, hash);

            // sanity check
            assert!(
                merkle_tree.root() == root,
                "sanity check failed, roots don't match"
            );

            next_leaf = max(next_leaf, leaf + 1);
        }

        let is_manager = ethereum.is_manager().await?;
        let nonce_offset = ethereum.get_nonce().await? - next_leaf;
        info!(
            "current nonce: {}, nonce offset: {}",
            ethereum.get_nonce().await?,
            nonce_offset
        );

        Ok(Self {
            ethereum,
            storage_file: options.storage_file,
            merkle_tree: RwLock::new(merkle_tree),
            next_leaf: AtomicUsize::new(next_leaf),
            tree_depth: options.tree_depth,
            is_manager,
            nonce_offset,
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
        // Check if manager
        if !self.is_manager {
            return Err(ServerError::NotManager);
        }

        // Fetch next leaf index
        let identity_index = self.next_leaf.fetch_add(1, Ordering::AcqRel);

        // Send Semaphore transaction, nonce calculated to ensure ordering
        self.ethereum
            .insert_identity(
                group_id,
                commitment,
                self.tree_depth,
                identity_index + self.nonce_offset,
            )
            .await?;

        // Update and write merkle tree
        {
            let mut merkle_tree = self.merkle_tree.write().await;
            merkle_tree.set(identity_index, *commitment);
        }

        self.store().await?;

        Ok(IndexResponse { identity_index })
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided index is out of bounds.
    #[instrument(level = "debug", skip_all)]
    pub async fn inclusion_proof(
        &self,
        _group_id: usize,
        identity_commitment: &Hash,
    ) -> Result<InclusionProofResponse, ServerError> {
        let merkle_tree = self.merkle_tree.read().await;
        let identity_index = match merkle_tree
            .leaves()
            .iter()
            .position(|&x| x == *identity_commitment)
        {
            Some(i) => i,
            None => return Err(ServerError::IdentityCommitmentNotFound),
        };

        let proof = merkle_tree.proof(identity_index);
        proof.map_or(Err(ServerError::IndexOutOfBounds), |proof| {
            Ok(InclusionProofResponse {
                root: merkle_tree.root(),
                proof,
            })
        })
    }

    #[instrument(level = "debug", skip_all)]
    async fn store(&self) -> EyreResult<()> {
        let file = File::create(&self.storage_file)?;
        let last_block = self.ethereum.last_block().await?;
        let next_leaf = self.next_leaf.load(Ordering::Acquire);
        let commitments = {
            let lock = self.merkle_tree.read().await;
            lock.leaves()[..next_leaf].to_vec()
        };
        let data = JsonCommitment {
            last_block,
            commitments,
        };
        serde_json::to_writer(BufWriter::new(file), &data)?;
        Ok(())
    }
}
