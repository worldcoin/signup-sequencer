use crate::mimc_tree::ExampleAlgorithm;
use anyhow::anyhow;
use merkletree::{merkle::MerkleTree, proof::Proof, store::VecStore};
use std::{
    convert::TryInto,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

const NUM_LEAVES: usize = 2;

pub type Commitment = [u8; 32];

pub fn initialize_commitments() -> Vec<Commitment> {
    let identity_commitments = vec![[0_u8; 32]; 1 << NUM_LEAVES];
    identity_commitments
}

pub fn inclusion_proof_helper(
    commitment: &str,
    commitments: &[Commitment],
) -> Result<Proof<[u8; 32]>, anyhow::Error> {
    // For some reason strings have extra `"`s on the ends
    let commitment = commitment.trim_matches('"');
    let commitment = hex::decode(commitment).unwrap();
    let commitment: [u8; 32] = (&commitment[..]).try_into().unwrap();
    let index = commitments
        .iter()
        .position(|x| *x == commitment)
        .ok_or(anyhow!("Commitment not found: {:?}", commitment))?;

    let t: MerkleTree<[u8; 32], ExampleAlgorithm, VecStore<_>> =
        MerkleTree::try_from_iter(commitments.clone().into_iter().map(|x| Ok(*x))).unwrap();
    t.gen_proof(index)
}

pub fn insert_identity_helper(
    commitment: &str,
    commitments: &mut [Commitment],
    index: Arc<AtomicUsize>,
) {
    let index: usize = index.fetch_add(1, Ordering::AcqRel);
    let commitment = commitment.trim_matches('"');
    let commitment = hex::decode(commitment).unwrap();
    let commitment: [u8; 32] = (&commitment[..]).try_into().unwrap();
    commitments[index] = commitment;
}
