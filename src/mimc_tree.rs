use crate::{
    merkle_tree::{self, Hasher, MerkleTree},
    mimc_hash::hash,
};
use zkp_u256::U256;

pub type Hash = [u8; 32];
pub type MimcTree = MerkleTree<MimcHash>;
pub type Branch = merkle_tree::Branch<MimcHash>;
pub type Proof = merkle_tree::Proof<MimcHash>;

pub struct MimcHash;

impl Hasher for MimcHash {
    type Hash = Hash;

    fn initial_leaf() -> Self::Hash {
        Self::Hash::default()
    }

    fn hash_node(left: &Self::Hash, right: &Self::Hash) -> Self::Hash {
        let left = U256::from_bytes_be(left);
        let right = U256::from_bytes_be(&right);
        hash(&[left, right]).to_bytes_be()
    }
}
