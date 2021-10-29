const NUM_LEAVES: usize = 20;

use std::{convert::TryInto, hash::Hasher, sync::{Arc, RwLock, atomic::AtomicUsize}};

use crypto::{
    digest::Digest,
    sha3::{Sha3, Sha3Mode},
};
use merkletree::{hash::Algorithm, merkle::MerkleTree, proof::Proof, store::VecStore};

pub struct ExampleAlgorithm(Sha3);

// TODO implement MiMC and various optimizations
impl ExampleAlgorithm {
    pub fn new() -> ExampleAlgorithm {
        ExampleAlgorithm(Sha3::new(Sha3Mode::Sha3_256))
    }
}

impl Default for ExampleAlgorithm {
    fn default() -> ExampleAlgorithm {
        ExampleAlgorithm::new()
    }
}

impl Hasher for ExampleAlgorithm {
    #[inline]
    fn write(&mut self, msg: &[u8]) {
        self.0.input(msg)
    }

    #[inline]
    fn finish(&self) -> u64 {
        unimplemented!()
    }
}

impl Algorithm<[u8; 32]> for ExampleAlgorithm {
    #[inline]
    fn hash(&mut self) -> [u8; 32] {
        let mut h = [0u8; 32];
        self.0.result(&mut h);
        h
    }

    #[inline]
    fn reset(&mut self) {
        self.0.reset();
    }
}

pub fn initialize_commitments() -> Vec<String> {
    let mut identity_commitments = vec![String::from(""); 2 ^ NUM_LEAVES];
    identity_commitments
}

pub fn inclusion_proof_helper(
    identiy_commitments: Vec<String>,
    index: usize,
) -> Result<Proof<[u8; 32]>, anyhow::Error> {
    // Convert all hex strings to [u8] for hashing -- TODO more efficient construction
    let t: MerkleTree<[u8; 32], ExampleAlgorithm, VecStore<_>> =
        MerkleTree::try_from_iter(identiy_commitments.into_iter().map(|x| {
            let hex_vec = hex::decode(x).unwrap();
            let z: [u8; 32] = (&hex_vec[..]).try_into().unwrap();
            Ok(z)
        }))
        .unwrap();
    t.gen_proof(index)
}

pub fn insert_identity_helper(
    identity_commitment: String,
    identity_commitments: Arc<RwLock<Vec<String>>>,
    index: Arc<AtomicUsize>,
) {
    let mut identity_commitments = identity_commitments.write().unwrap().to_vec();
    // identity_commitments[index]= identity_commitment;
}
