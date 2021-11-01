const NUM_LEAVES: usize = 20;

use std::{convert::TryInto, sync::{Arc, RwLock, atomic::{AtomicUsize, Ordering}}};

use merkletree::{merkle::MerkleTree, proof::Proof, store::VecStore};

use crate::mimc_tree::ExampleAlgorithm;

pub fn initialize_commitments() -> Vec<String> {
    let identity_commitments = vec![String::from(""); 2 ^ NUM_LEAVES];
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
    let index: usize = index.fetch_add(1, Ordering::AcqRel);
    identity_commitments[index]= identity_commitment;
}
