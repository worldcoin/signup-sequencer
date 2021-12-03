use crate::{
    ethereum::{self, Ethereum},
    hash::Hash,
    mimc_tree::{MimcTree, Proof},
    server::Error as ServerError,
};
use core::cmp::max;
use eyre::Result as EyreResult;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use structopt::StructOpt;
use tokio::sync::RwLock;
use tracing::{info, warn};

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

#[derive(Clone, Debug, PartialEq, StructOpt)]
pub struct Options {
    #[structopt(flatten)]
    pub ethereum: ethereum::Options,

    /// Storage location for the Merkle tree.
    #[structopt(long, env, default_value = "commitments.json")]
    pub storage_file: PathBuf,

    /// Number of layers in the tree. Defaults to 21 to match Semaphore.sol
    /// defaults.
    #[structopt(long, env, default_value = "21")]
    pub tree_depth: usize,

    /// Initial value of the Merkle tree leaves. Defaults to the initial value
    /// in Semaphore.sol.
    #[structopt(
        long,
        env,
        default_value = "1c4823575d154474ee3e5ac838d002456a815181437afd14f126da58a9912bbe"
    )]
    pub initial_leaf: Hash,
}

pub struct App {
    ethereum:     Ethereum,
    storage_file: PathBuf,
    merkle_tree:  RwLock<MimcTree>,
    next_leaf:    AtomicUsize,
}

impl App {
    /// # Errors
    ///
    /// Will return `Err` if the internal Ethereum handler errors or if the
    /// `options.storage_file` is not accessible.
    pub async fn new(options: Options) -> EyreResult<Self> {
        let ethereum = Ethereum::new(options.ethereum).await?;
        let mut merkle_tree = MimcTree::new(options.tree_depth, options.initial_leaf);

        // Read tree from file
        info!(path = ?&options.storage_file, "Reading tree from storage");
        let (mut next_leaf, last_block) = if options.storage_file.is_file() {
            let file = File::open(&options.storage_file)?;
            if file.metadata()?.len() > 0 {
                let file: JsonCommitment = serde_json::from_reader(file)?;
                let next_leaf = file.commitments.len();
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
        let events = ethereum.fetch_events(last_block).await?;
        for (leaf, hash) in events {
            merkle_tree.set(leaf, hash);
            next_leaf = max(next_leaf, leaf + 1);
        }

        Ok(Self {
            ethereum,
            storage_file: options.storage_file,
            merkle_tree: RwLock::new(merkle_tree),
            next_leaf: AtomicUsize::new(next_leaf),
        })
    }

    /// # Errors
    ///
    /// Will return `Err` if the Eth handler cannot insert the identity to the
    /// contract, or if writing to the storage file fails.
    pub async fn insert_identity(&self, commitment: &Hash) -> Result<IndexResponse, ServerError> {
        // Send Semaphore transaction
        self.ethereum.insert_identity(commitment).await?;

        // Update merkle tree
        let identity_index;
        {
            let mut merkle_tree = self.merkle_tree.write().await;
            identity_index = self.next_leaf.fetch_add(1, Ordering::AcqRel);
            merkle_tree.set(identity_index, *commitment);
        }

        // Write state file
        self.store().await?;

        Ok(IndexResponse { identity_index })
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided index is out of bounds.
    pub async fn inclusion_proof(&self, identity_index: usize) -> Result<Proof, ServerError> {
        let merkle_tree = self.merkle_tree.read().await;
        let proof = merkle_tree.proof(identity_index);

        proof.ok_or(ServerError::IndexOutOfBounds)
    }

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
        serde_json::to_writer(&file, &data)?;
        Ok(())
    }
}
