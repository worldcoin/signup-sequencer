const NUM_LEAVES: usize = 20;

use anyhow::anyhow;
use std::{convert::TryInto, num::ParseIntError, sync::{Arc, RwLock, atomic::{AtomicUsize, Ordering}}};

use merkletree::{merkle::MerkleTree, proof::Proof, store::VecStore};

use crate::mimc_tree::ExampleAlgorithm;

pub fn initialize_commitments() -> Vec<String> {
    let identity_commitments = vec![String::from(""); 2 << (NUM_LEAVES - 1)];
    identity_commitments
}

pub fn decode_hex(s: &str) -> Result<Vec<u8>, ParseIntError> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect()
}

pub fn inclusion_proof_helper(
    commitment: String,
    commitments: Arc<RwLock<Vec<String>>>,
) -> Result<Proof<[u8; 32]>, anyhow::Error> {
    let commitments = commitments.read().unwrap();
    println!("First elemnt {}, commitment {}, equal? {}", commitments[0], commitment, commitments[0] == commitment);
    let index = match commitments.iter().position(|x| *x == commitment) {
        Some(index) => index,
        None => return Err(anyhow!("Commitment not found: {}", commitment)),
    };
    println!("Index {}", index);
    println!("Vec length {}: {} : {}", commitments.len(), 2 << (NUM_LEAVES - 1), i32::from(2).pow(20));

    // Convert all hex strings to [u8] for hashing -- TODO more efficient construction
    let t: MerkleTree<[u8; 32], ExampleAlgorithm, VecStore<_>> =
        MerkleTree::try_from_iter(commitments.clone().into_iter().map(|x| {
            let x = if x != "" {
                // For some reason strings have extra `"`s on the ends
                x.trim_matches('"')
            } else {
                // TODO: Zero value
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            };
            let hex_vec = decode_hex(&x).unwrap();
            let z: [u8; 32] = (&hex_vec[..]).try_into().unwrap();
            Ok(z)
        }))
        .unwrap();
    t.gen_proof(index)
}

pub fn insert_identity_helper(
    commitment: String,
    commitments: Arc<RwLock<Vec<String>>>,
    index: Arc<AtomicUsize>,
) {
    let mut commitments = commitments.write().unwrap();
    let index: usize = index.fetch_add(1, Ordering::AcqRel);
    commitments[index]= commitment;
}
