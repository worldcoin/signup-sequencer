use ruint::uint;
use semaphore::Field;

use crate::identity_tree::Hash;

// See <https://docs.rs/ark-bn254/latest/ark_bn254>
pub const MODULUS: Field =
    uint!(21888242871839275222246405745257275088548364400416034343698204186575808495617_U256);

pub struct IdentityValidator {
    snark_scalar_field: Hash,
}

// TODO Export the reduced-ness check that this is enabling from the
//  `semaphore-rs` library when we bump the version.
impl IdentityValidator {
    pub fn new() -> Self {
        Self {
            snark_scalar_field: Hash::from(MODULUS),
        }
    }

    pub fn identity_is_reduced(&self, commitment: Hash) -> bool {
        commitment.lt(&self.snark_scalar_field)
    }
}

impl Default for IdentityValidator {
    fn default() -> Self {
        Self::new()
    }
}
