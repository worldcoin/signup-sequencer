use ethers::types::U256;
use serde::{Deserialize, Serialize};

/// A representation of an identity insertion into the merkle tree as used for
/// the prover endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The identity commitment value that is inserted into the merkle tree.
    pub commitment: U256,

    /// The merkle proof for the insertion of `commitment` into the merkle tree.
    pub merkle_proof: MerkleProof,
}

impl Identity {
    /// Constructs a new identity value from the provided `commitment` and
    /// `merkle_proof`.
    pub fn new(commitment: U256, merkle_proof: MerkleProof) -> Self {
        Self {
            commitment,
            merkle_proof,
        }
    }
}

/// A merkle proof is a list of values for the nodes in the merkle tree once the
/// new identity has been inserted.
///
/// # Important Note
///
/// These nodes should be ordered with the value closest to the leaf with the
/// identity as the 0-th element and the n-th element as the root value of the
/// tree.
pub type MerkleProof = Vec<U256>;
