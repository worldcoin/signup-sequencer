use crate::mimc_tree::ExampleAlgorithm;
use anyhow::anyhow;
use ethers::prelude::{Address, Http, LocalWallet, Middleware, Provider, Signer, SignerMiddleware, abigen};
use std::convert::TryFrom;
use std::sync::RwLock;
use std::{convert::TryInto, sync::{Arc, atomic::{AtomicUsize, Ordering}}};
use merkletree::{merkle::MerkleTree, proof::Proof, store::VecStore};

// TODO Use real value
const NUM_LEAVES: usize = 2;

const SEMAPHORE_ADDRESS: &str = "0x1a2BdAE39EB03E1D10551866717F8631bEf6e88a";
const WALLET_CLAIMS_ADDRESS: &str = "0x39777E5d6bB83F4bF51fa832cD50E3c74eeA50A5";

pub type Commitment = [u8; 32];

abigen!(
    Semaphore,
    "./solidity/abi/semaphore_abi.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

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
        .ok_or_else(|| anyhow!("Commitment not found: {:?}", commitment))?;

    let t: MerkleTree<[u8; 32], ExampleAlgorithm, VecStore<_>> =
        MerkleTree::try_from_iter(commitments.iter().map(|x| Ok(*x))).unwrap();
    t.gen_proof(index)
}

pub async fn insert_identity_helper(
    commitment: &str,
    commitments: Arc<RwLock<Vec<Commitment>>>,
    index: &Arc<AtomicUsize>,
) -> Result<bool, anyhow::Error> {
    let commitment = commitment.trim_matches('"');
    let decoded_commitment = hex::decode(commitment).unwrap();
    let commitment: [u8; 32] = (&decoded_commitment[..]).try_into().unwrap();
    {
        let mut commitments = commitments.write().unwrap();
        let index: usize = index.fetch_add(1, Ordering::AcqRel);
        commitments[index] = commitment;
    }

    let provider = Provider::<Http>::try_from(
        "http://localhost:8545"
    ).expect("could not instantiate HTTP Provider");
    let chain_id = provider.get_chainid().await.unwrap();

    let wallet = "ee79b5f6e221356af78cf4c36f4f7885a11b67dfcc81c34d80249947330c0f82".parse::<LocalWallet>()?;
    let wallet = wallet.with_chain_id(chain_id.as_u64());

    let signer = SignerMiddleware::new(provider, wallet.clone());
    let signer = Arc::new(signer);

    let semaphore_address = SEMAPHORE_ADDRESS.parse::<Address>().unwrap();
    let semaphore_contract = Semaphore::new(
        semaphore_address,
        signer.clone(),
    );


    let tx = semaphore_contract.insert_identity(
        decoded_commitment[..].into(),
    );
    let pending_tx = signer.send_transaction(tx.tx, None).await.unwrap();
    let res = pending_tx.await?.unwrap();
    println!("Inserted identity {:?}", res);
    Ok(true)
}
