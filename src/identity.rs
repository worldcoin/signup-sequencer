use crate::{
    mimc_tree::{Hash, MimcTree},
    solidity::{ContractSigner, JsonCommitment, SemaphoreContract, COMMITMENTS_FILE},
};
use ethers::prelude::Middleware;
use eyre::Result as EyreResult;
use std::{convert::TryInto, fs::File};

pub type Commitment = Hash;

pub async fn insert_identity_commitment(
    tree: &mut MimcTree,
    signer: &ContractSigner,
    commitment: &str,
    index: usize,
) -> EyreResult<()> {
    let decoded_commitment = hex::decode(commitment)?;
    let commitment: Commitment = (&decoded_commitment[..]).try_into()?;
    tree.set(index, commitment);
    let num = signer.get_block_number().await?;
    serde_json::to_writer(&File::create(COMMITMENTS_FILE)?, &JsonCommitment {
        last_block:  num.as_usize(),
        commitments: tree.leaves()[..=index].to_vec(),
    })?;
    Ok(())
}

pub async fn insert_identity_to_contract(
    semaphore_contract: &SemaphoreContract,
    signer: &ContractSigner,
    commitment: &str,
) -> EyreResult<bool> {
    let decoded_commitment = hex::decode(commitment).unwrap();
    let tx = semaphore_contract.insert_identity(decoded_commitment[..].into());
    let pending_tx = signer.send_transaction(tx.tx, None).await.unwrap();
    pending_tx.await?.unwrap();
    Ok(true)
}
