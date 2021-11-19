use crate::{
    ethereum::{self, Ethereum},
    hash::Hash,
    mimc_tree::MimcTree,
    server::Error,
};
use core::cmp::max;
use eyre::Result as EyreResult;
use hyper::{Body, Response};
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

#[derive(Debug, PartialEq, StructOpt)]
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
    last_leaf:    AtomicUsize, // TODO: Is this last or next leaf?
}

impl App {
    pub async fn new(options: Options) -> EyreResult<Self> {
        let ethereum = Ethereum::new(options.ethereum).await?;
        let mut merkle_tree = MimcTree::new(options.tree_depth, options.initial_leaf);

        // Read tree from file
        info!(path = ?&options.storage_file, "Reading tree from storage");
        let (mut last_leaf, last_block) = if options.storage_file.is_file() {
            let file = File::open(&options.storage_file)?;
            let file: JsonCommitment = serde_json::from_reader(file)?;
            let last_leaf = file.commitments.len();
            merkle_tree.set_range(0, file.commitments);
            (last_leaf, file.last_block)
        } else {
            warn!(path = ?&options.storage_file, "Storage file not found, skipping.");
            (0, 0)
        };

        // Read events from blockchain
        let events = ethereum.fetch_events(last_block).await?;
        for (leaf, hash) in events {
            merkle_tree.set(leaf, hash);
            last_leaf = max(last_leaf, leaf + 1);
        }

        Ok(Self {
            ethereum,
            storage_file: options.storage_file,
            merkle_tree: RwLock::new(merkle_tree),
            last_leaf: AtomicUsize::new(last_leaf),
        })
    }

    pub async fn insert_identity(&self, commitment: &Hash) -> Result<Response<Body>, Error> {
        // Update merkle tree
        {
            let mut merkle_tree = self.merkle_tree.write().await;
            let last_leaf = self.last_leaf.fetch_add(1, Ordering::AcqRel);
            merkle_tree.set(last_leaf, *commitment);
        }

        // Write state file
        self.store().await?;

        // Send Semaphore transaction
        self.ethereum.insert_identity(commitment).await?;

        Ok(Response::new("Insert Identity!\n".into()))
    }

    #[allow(clippy::unused_async)]
    pub async fn inclusion_proof(&self, commitment: &Hash) -> Result<Response<Body>, Error> {
        let merkle_tree = self.merkle_tree.read().await;
        let proof = merkle_tree
            .position(commitment)
            .map(|i| merkle_tree.proof(i));

        println!("Proof: {:?}", proof);
        // TODO handle commitment not found
        let response = "Inclusion Proof!\n"; // TODO: proof
        Ok(Response::new(response.into()))
    }

    pub async fn store(&self) -> EyreResult<()> {
        let file = File::create(&self.storage_file)?;
        let last_block = self.ethereum.last_block().await?;
        let last_leaf = self.last_leaf.load(Ordering::Acquire);
        let commitments = {
            let lock = self.merkle_tree.read().await;
            lock.leaves()[..=last_leaf].to_vec()
        };
        let data = JsonCommitment {
            last_block,
            commitments,
        };
        serde_json::to_writer(&file, &data)?;
        Ok(())
    }
}
