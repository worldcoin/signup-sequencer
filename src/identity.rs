use crate::mimc_tree::{Hash, MimcTree, Proof};
use ethers::prelude::{
    abigen, Address, Http, LocalWallet, Middleware, Provider, Signer, SignerMiddleware,
};
use eyre::{bail, Error as EyreError};
use std::{
    convert::{TryFrom, TryInto},
    sync::Arc,
};

pub type Commitment = Hash;

const SEMAPHORE_ADDRESS: &str = "0x762403528A6917587f45aD9ec18513244f8DD87e";
// const WALLET_CLAIMS_ADDRESS: &str =
// "0x39777E5d6bB83F4bF51fa832cD50E3c74eeA50A5";

abigen!(
    Semaphore,
    "./src/abis/semaphore_abi.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

pub fn inclusion_proof_helper(tree: &MimcTree, commitment: &str) -> Result<Proof, EyreError> {
    let decoded_commitment = hex::decode(commitment).unwrap();
    let decoded_commitment: [u8; 32] = (&decoded_commitment[..]).try_into().unwrap();
    if let Some(index) = tree.position(&decoded_commitment) {
        return Ok(tree.proof(index));
    }
    bail!("Commitment not found {}", commitment);
}

pub fn insert_identity_commitment(tree: &mut MimcTree, commitment: &str, index: usize) {
    // let commitment = commitment.trim_matches('"');
    let decoded_commitment = hex::decode(commitment).unwrap();
    let commitment: [u8; 32] = (&decoded_commitment[..]).try_into().unwrap();
    tree.set(index, commitment);
}

pub async fn insert_identity_to_contract(commitment: &str) -> Result<bool, EyreError> {
    let decoded_commitment = hex::decode(commitment).unwrap();

    let provider = Provider::<Http>::try_from("http://localhost:8545")
        .expect("could not instantiate HTTP Provider");
    let chain_id = provider.get_chainid().await.unwrap();

    let wallet = "ee79b5f6e221356af78cf4c36f4f7885a11b67dfcc81c34d80249947330c0f82"
        .parse::<LocalWallet>()?;
    let wallet = wallet.with_chain_id(chain_id.as_u64());

    let signer = SignerMiddleware::new(provider, wallet.clone());
    let signer = Arc::new(signer);

    let semaphore_address = SEMAPHORE_ADDRESS.parse::<Address>().unwrap();
    let semaphore_contract = Semaphore::new(semaphore_address, signer.clone());

    let tx = semaphore_contract.insert_identity(decoded_commitment[..].into());
    let pending_tx = signer.send_transaction(tx.tx, None).await.unwrap();
    let res = pending_tx.await?.unwrap();
    println!("Inserted identity {:?}", res);
    Ok(true)
}
