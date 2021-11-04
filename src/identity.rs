const NUM_LEAVES: usize = 20;

use anyhow::anyhow;
use ethers::prelude::{Address, Http, Provider, abigen};
use std::convert::TryFrom;
use std::{convert::TryInto, num::ParseIntError, sync::{Arc, RwLock, atomic::{AtomicUsize, Ordering}}};

use merkletree::{merkle::MerkleTree, proof::Proof, store::VecStore};

use crate::mimc_tree::ExampleAlgorithm;

const SEMAPHORE_ADDRESS: &str = "0xB0136923cb6295b2908618166b1b206493ce3F1a";
const WALLET_CLAIMS_ADDRESS: &str = "0x39777E5d6bB83F4bF51fa832cD50E3c74eeA50A5";

pub fn initialize_commitments() -> Vec<String> {
    let identity_commitments = vec![String::from(""); 1 << NUM_LEAVES];
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
    let index = match commitments.iter().position(|x| *x == commitment) {
        Some(index) => index,
        None => return Err(anyhow!("Commitment not found: {}", commitment)),
    };

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

pub async fn insert_identity_helper(
    commitment: String,
    commitments: Arc<RwLock<Vec<String>>>,
    index: Arc<AtomicUsize>,
) -> Result<bool, anyhow::Error> {
    abigen!(
        Semaphore,
        "./solidity/abi/semaphore_abi.json",
    );
    {
        let mut commitments = commitments.write().unwrap();
        let index: usize = index.fetch_add(1, Ordering::AcqRel);
        commitments[index]= commitment.clone();
    }

    let provider = Provider::<Http>::try_from(
        "http://localhost:8545"
    ).expect("could not instantiate HTTP Provider");

    // TODO add signer middleware
    // // We have to re-create the contract here with a signer instead of
    // // provider, to submit a transaction :(
    // let signer = SignerMiddleware::new(provider, signer);
    // let contract = Contract::new(opts.org, abi, signer);
    // let call = contract.method::<_, ()>("anchor", (id, tag, hash))?;

    let semaphore_address = SEMAPHORE_ADDRESS.parse::<Address>().unwrap();
    let semaphore_contract = Semaphore::new(
        semaphore_address,
        Arc::new(provider),
    );

    let commitment = commitment.trim_matches('"');

    let decoded_commitment = hex::decode(commitment).unwrap();
    println!("Starting insert {:?}", decoded_commitment);
    semaphore_contract.insert_identity(
        decoded_commitment[..].into(),
    ).call().await?;
    println!("Inserted identity");
    Ok(true)
}
