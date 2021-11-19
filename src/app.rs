use crate::{
    ethereum::{
        initialize_semaphore, parse_identity_commitments, ContractSigner, SemaphoreContract,
    },
    hash::Hash,
    mimc_tree::MimcTree,
    server::Error,
};
use ethers::prelude::*;
use eyre::{eyre, Result as EyreResult};
use hyper::{Body, Response};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use structopt::StructOpt;
use tokio::sync::RwLock;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonCommitment {
    pub last_block:  usize,
    pub commitments: Vec<Hash>,
}

#[derive(Debug, PartialEq, StructOpt)]
pub struct Options {
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
    storage_file:       PathBuf,
    merkle_tree:        RwLock<MimcTree>,
    last_leaf:          AtomicUsize,
    signer:             ContractSigner,
    semaphore_contract: SemaphoreContract,
}

impl App {
    pub async fn new(options: Options) -> EyreResult<Self> {
        let (signer, semaphore) = initialize_semaphore().await?;
        let mut merkle_tree = MimcTree::new(options.tree_depth, options.initial_leaf);
        let last_leaf =
            parse_identity_commitments(&options.storage_file, &mut merkle_tree, semaphore.clone())
                .await?;
        Ok(Self {
            storage_file: options.storage_file,
            merkle_tree: RwLock::new(merkle_tree),
            last_leaf: AtomicUsize::new(last_leaf),
            signer,
            semaphore_contract: semaphore,
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
        let tx = self.semaphore_contract.insert_identity(commitment.into());
        let pending_tx = self.signer.send_transaction(tx.tx, None).await.unwrap();
        let _receipt = pending_tx.await.map_err(|e| eyre!(e))?;
        // TODO: What does it mean if `_receipt` is None?

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
        let last_block = self.signer.get_block_number().await?.as_usize();
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
