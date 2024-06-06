use anyhow::anyhow;

use crate::config::Config;
use crate::prover::identity::Identity;

pub struct ProofValidator {
    tree_depth: usize,
}

impl ProofValidator {
    pub fn new(config: &Config) -> Self {
        Self {
            tree_depth: config.tree.tree_depth,
        }
    }

    /// Validates that merkle proofs are of the correct length against tree
    /// depth
    pub fn validate_merkle_proofs(&self, identity_commitments: &[Identity]) -> anyhow::Result<()> {
        for id in identity_commitments {
            if id.merkle_proof.len() != self.tree_depth {
                return Err(anyhow!(format!(
                    "Length of merkle proof ({len}) did not match tree depth ({depth})",
                    len = id.merkle_proof.len(),
                    depth = self.tree_depth
                )));
            }
        }

        Ok(())
    }
}
