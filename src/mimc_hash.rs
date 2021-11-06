//! Hash function compatible with Semaphore's Merkle tree hash function
//!
//! See <https://github.com/appliedzkp/semaphore/blob/master/circuits/circom/semaphore-base.circom#L10>
//! See <https://github.com/kobigurk/circomlib/blob/4284dc1ef984a204db08864f5da530c97f9376ef/circuits/mimcsponge.circom>
//! See <https://github.com/iden3/circomlibjs/blob/main/src/mimcsponge.js>

use ethers::utils::keccak256;
use once_cell::sync::Lazy;
use zkp_u256::U256;

const NUM_ROUNDS: usize = 220;

static MODULUS: Lazy<U256> = Lazy::new(|| {
    U256::from_decimal_str(
        "21888242871839275222246405745257275088548364400416034343698204186575808495617",
    )
    .unwrap()
});

static ROUND_CONSTANTS: Lazy<[U256; NUM_ROUNDS]> = Lazy::new(|| {
    const SEED: &str = "mimcsponge";
    let mut result = [U256::ZERO; NUM_ROUNDS];
    let mut bytes = keccak256(SEED.as_bytes());
    for constant in result[1..NUM_ROUNDS - 1].iter_mut() {
        bytes = keccak256(&bytes);
        *constant = U256::from_bytes_be(&bytes);
        *constant %= &*MODULUS;
    }
    result
});

#[cfg(test)]
pub mod test {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn correct_round_constants() {
        // See <https://github.com/kobigurk/circomlib/blob/4284dc1ef984a204db08864f5da530c97f9376ef/circuits/mimcsponge.circom#L44>
        assert_eq!(ROUND_CONSTANTS[0], U256::ZERO);
        assert_eq!(
            ROUND_CONSTANTS[1],
            U256::from_decimal_str(
                "7120861356467848435263064379192047478074060781135320967663101236819528304084"
            )
            .unwrap()
        );
        assert_eq!(
            ROUND_CONSTANTS[2],
            U256::from_decimal_str(
                "5024705281721889198577876690145313457398658950011302225525409148828000436681"
            )
            .unwrap()
        );
        assert_eq!(
            ROUND_CONSTANTS[218],
            U256::from_decimal_str(
                "2119542016932434047340813757208803962484943912710204325088879681995922344971"
            )
            .unwrap()
        );
        assert_eq!(ROUND_CONSTANTS[219], U256::ZERO);
    }
}

#[cfg(feature = "bench")]
pub mod bench {
    use criterion::Criterion;

    pub fn group(criterion: &mut Criterion) {}
}
