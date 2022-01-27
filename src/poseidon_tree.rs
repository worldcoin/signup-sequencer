use crate::{
    hash::Hash,
    merkle_tree::{self, Hasher, MerkleTree},
};
use ff::{PrimeField, PrimeFieldRepr};
use once_cell::sync::Lazy;
use poseidon_rs::{Fr, FrRepr, Poseidon};
use serde::Serialize;

static POSEIDON: Lazy<Poseidon> = Lazy::new(Poseidon::new);

#[allow(dead_code)]
pub type PoseidonTree = MerkleTree<PoseidonHash>;
#[allow(dead_code)]
pub type Branch = merkle_tree::Branch<PoseidonHash>;
#[allow(dead_code)]
pub type Proof = merkle_tree::Proof<PoseidonHash>;

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PoseidonHash;

#[allow(clippy::fallible_impl_from)] // TODO
impl From<&Hash> for Fr {
    fn from(hash: &Hash) -> Self {
        let mut repr = FrRepr::default();
        repr.read_be(&hash.as_bytes_be()[..]).unwrap();
        Self::from_repr(repr).unwrap()
    }
}

#[allow(clippy::fallible_impl_from)] // TODO
impl From<Fr> for Hash {
    fn from(fr: Fr) -> Self {
        let mut bytes = [0_u8; 32];
        fr.into_repr().write_be(&mut bytes[..]).unwrap();
        Self::from_bytes_be(bytes)
    }
}

impl Hasher for PoseidonHash {
    type Hash = Hash;

    fn hash_node(left: &Self::Hash, right: &Self::Hash) -> Self::Hash {
        POSEIDON
            .hash(vec![left.into(), right.into()])
            .unwrap() // TODO
            .into()
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use hex_literal::hex;

    #[test]
    fn test_tree_4() {
        const LEAF: Hash = Hash::from_bytes_be(hex!(
            "1c4823575d154474ee3e5ac838d002456a815181437afd14f126da58a9912bbe"
        ));

        let tree = PoseidonTree::new(3, LEAF);
        assert_eq!(tree.num_leaves(), 4);
        assert_eq!(
            tree.root(),
            Hash::from_bytes_be(hex!(
                "2413c857992af7eef8e920e52cb42997f461640b1897caed2101c7ccbf3b12b1"
            ))
        );
        let proof = tree.proof(3).expect("proof should exist");
        assert_eq!(
            proof,
            crate::merkle_tree::Proof(vec![
                Branch::Right(LEAF),
                Branch::Right(Hash::from_bytes_be(hex!(
                    "0f51fdd2d74e52adb1b467b9334f88ce595f5426dea8f9509be267952c846f92"
                ))),
            ])
        );
    }
}
