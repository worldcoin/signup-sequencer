use ethers::types::U256;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};

use crate::prover::identity::Identity;
use crate::prover::Prover;

/// The proof term returned from the `semaphore-mtb` proof generation service.
///
/// The names of the data fields match those from the JSON response exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    pub ar: [U256; 2],
    pub bs: [[U256; 2]; 2],
    pub krs: [U256; 2],
}

impl From<[U256; 8]> for Proof {
    fn from(value: [U256; 8]) -> Self {
        Self {
            ar: [value[0], value[1]],
            bs: [[value[2], value[3]], [value[4], value[5]]],
            krs: [value[6], value[7]],
        }
    }
}

impl From<Proof> for [U256; 8] {
    fn from(value: Proof) -> Self {
        [
            value.ar[0],
            value.ar[1],
            value.bs[0][0],
            value.bs[0][1],
            value.bs[1][0],
            value.bs[1][1],
            value.krs[0],
            value.krs[1],
        ]
    }
}

#[instrument(level = "debug", skip(prover, identity_commitments))]
pub async fn prepare_insertion_proof(
    prover: &Prover,
    start_index: usize,
    pre_root: U256,
    identity_commitments: &[Identity],
    post_root: U256,
) -> anyhow::Result<Proof> {
    let batch_size = identity_commitments.len();

    let actual_start_index: u32 = start_index.try_into()?;

    info!(
        "Sending {} identities to prover of batch size {}",
        batch_size,
        prover.batch_size()
    );

    let proof_data: Proof = prover
        .generate_insertion_proof(
            actual_start_index,
            pre_root,
            post_root,
            identity_commitments,
        )
        .await?;

    Ok(proof_data)
}

#[instrument(level = "debug", skip(prover, identity_commitments))]
pub async fn prepare_deletion_proof(
    prover: &Prover,
    pre_root: U256,
    deletion_indices: Vec<u32>,
    identity_commitments: Vec<Identity>,
    post_root: U256,
) -> anyhow::Result<Proof> {
    info!(
        "Sending {} identities to prover of batch size {}",
        identity_commitments.len(),
        prover.batch_size()
    );

    let proof_data: Proof = prover
        .generate_deletion_proof(pre_root, post_root, deletion_indices, identity_commitments)
        .await?;

    Ok(proof_data)
}
