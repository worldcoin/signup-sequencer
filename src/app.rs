use crate::{
    hash::Hash,
    identity::{inclusion_proof_helper, insert_identity_commitment, insert_identity_to_contract},
    mimc_tree::MimcTree,
    server::Error,
    solidity::{
        initialize_semaphore, parse_identity_commitments, ContractSigner, SemaphoreContract,
    },
};
use eyre::Result as EyreResult;
use hex_literal::hex;
use hyper::{Body, Response};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::RwLock;

const NOTHING_UP_MY_SLEEVE: Hash = Hash::from_bytes_be(hex!(
    "1c4823575d154474ee3e5ac838d002456a815181437afd14f126da58a9912bbe"
));

pub struct App {
    merkle_tree:        RwLock<MimcTree>,
    last_leaf:          AtomicUsize,
    signer:             ContractSigner,
    semaphore_contract: SemaphoreContract,
}

impl App {
    pub async fn new(depth: usize) -> EyreResult<Self> {
        let (signer, semaphore) = initialize_semaphore().await?;
        let mut merkle_tree = MimcTree::new(depth, NOTHING_UP_MY_SLEEVE);
        let last_leaf = parse_identity_commitments(&mut merkle_tree, semaphore.clone()).await?;
        Ok(Self {
            merkle_tree: RwLock::new(merkle_tree),
            last_leaf: AtomicUsize::new(last_leaf),
            signer,
            semaphore_contract: semaphore,
        })
    }

    #[allow(clippy::unused_async)]
    pub async fn inclusion_proof(&self, commitment: &Hash) -> Result<Response<Body>, Error> {
        let merkle_tree = self.merkle_tree.read().await;
        let proof = inclusion_proof_helper(&merkle_tree, commitment);
        println!("Proof: {:?}", proof);
        // TODO handle commitment not found
        let response = "Inclusion Proof!\n"; // TODO: proof
        Ok(Response::new(response.into()))
    }

    pub async fn insert_identity(&self, commitment: &Hash) -> Result<Response<Body>, Error> {
        {
            let mut merkle_tree = self.merkle_tree.write().await;
            let last_leaf = self.last_leaf.fetch_add(1, Ordering::AcqRel);
            insert_identity_commitment(&mut merkle_tree, &self.signer, commitment, last_leaf)
                .await?;
        }

        insert_identity_to_contract(&self.semaphore_contract, &self.signer, commitment).await?;
        Ok(Response::new("Insert Identity!\n".into()))
    }
}
