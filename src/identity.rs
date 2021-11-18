use crate::{
    hash::Hash,
    mimc_tree::{MimcTree, Proof},
    solidity::{ContractSigner, JsonCommitment, SemaphoreContract, COMMITMENTS_FILE},
};
use ethers::prelude::{Middleware, U256};
use eyre::{bail, Error as EyreError, Result as EyreResult};
use std::fs::File;

pub type Commitment = Hash;

impl From<&Hash> for U256 {
    fn from(hash: &Hash) -> Self {
        Self::from_big_endian(hash.as_bytes_be())
    }
}

impl From<U256> for Hash {
    fn from(u256: U256) -> Self {
        let mut bytes = [0_u8; 32];
        u256.to_big_endian(&mut bytes);
        Self::from_bytes_be(bytes)
    }
}

pub fn inclusion_proof_helper(
    tree: &MimcTree,
    commitment: &Commitment,
) -> Result<Proof, EyreError> {
    if let Some(index) = tree.position(commitment) {
        return Ok(tree.proof(index));
    }
    bail!("Commitment not found {:?}", commitment);
}

pub async fn insert_identity_commitment(
    tree: &mut MimcTree,
    signer: &ContractSigner,
    commitment: &Hash,
    index: usize,
) -> EyreResult<()> {
    tree.set(index, *commitment);
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
    commitment: &Hash,
) -> EyreResult<bool> {
    let tx = semaphore_contract.insert_identity(commitment.into());
    let pending_tx = signer.send_transaction(tx.tx, None).await.unwrap();
    pending_tx.await?.unwrap();
    Ok(true)
}
