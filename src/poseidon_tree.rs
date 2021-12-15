use crate::{
    hash::Hash,
    merkle_tree::{self, Hasher, MerkleTree},
};
use ff::*;
use num::{bigint::BigInt, Num};
use once_cell::sync::Lazy;
use poseidon_rs::{Fr, Poseidon};
use serde::Serialize;

static POSEIDON: Lazy<Poseidon> = Lazy::new(|| Poseidon::new());

pub type PoseidonTree = MerkleTree<PoseidonHash>;
#[allow(dead_code)]
pub type Branch = merkle_tree::Branch<PoseidonHash>;
#[allow(dead_code)]
pub type Proof = merkle_tree::Proof<PoseidonHash>;

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PoseidonHash;

impl Hasher for PoseidonHash {
    type Hash = Hash;

    fn hash_node(left: &Self::Hash, right: &Self::Hash) -> Self::Hash {
        let left_bi = BigInt::from_str_radix(&hex::encode(left.as_bytes_be()), 16).unwrap();
        let left_fr = Fr::from_str(&left_bi.to_str_radix(10)).unwrap();

        let right_bi = BigInt::from_str_radix(&hex::encode(right.as_bytes_be()), 16).unwrap();
        let right_fr = Fr::from_str(&right_bi.to_str_radix(10)).unwrap();

        let hash = POSEIDON.hash(vec![left_fr, right_fr]).unwrap();

        let ret = hex::decode(to_hex(&hash)).unwrap();
        let mut d: [u8; 32] = Default::default();
        d.copy_from_slice(&ret[0..32]);
        Hash::from_bytes_be(d)
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use hex_literal::hex;

    #[test]
    fn test_tree_4() {
        const LEAF: Hash = Hash::from_bytes_be(hex!(
            "0000000000000000000000000000000000000000000000000000000000000000"
        ));

        let tree = PoseidonTree::new(3, LEAF);
        assert_eq!(tree.num_leaves(), 4);
        assert_eq!(
            tree.root(),
            Hash::from_bytes_be(hex!(
                "1069673dcdb12263df301a6ff584a7ec261a44cb9dc68df067a4774460b1f1e1"
            ))
        );
        let proof = tree.proof(3).expect("proof should exist");
        assert_eq!(
            proof,
            crate::merkle_tree::Proof(vec![
                Branch::Right(LEAF),
                Branch::Right(Hash::from_bytes_be(hex!(
                    "2098f5fb9e239eab3ceac3f27b81e481dc3124d55ffed523a839ee8446b64864"
                ))),
            ])
        );
    }
}
