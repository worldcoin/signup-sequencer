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

// TODO: Tests with MimcHash

#[cfg(feature = "bench")]
pub(crate) mod bench {
    use super::*;
    use criterion::{black_box, Criterion};

    // TODO: Randomize trees and indices
    // TODO: Bench over a range of depths

    const DEPTH: usize = 20;

    pub(crate) fn group(criterion: &mut Criterion) {
        bench_set(criterion);
        bench_proof(criterion);
        bench_verify(criterion);
    }

    fn bench_set(criterion: &mut Criterion) {
        let mut tree = MimcTree::new(DEPTH);
        let index = 354184;
        let hash = [0_u8; 32];
        criterion.bench_function("mimc_tree_set", move |bencher| {
            bencher.iter(|| tree.set(index, black_box(hash)));
        });
    }

    fn bench_proof(criterion: &mut Criterion) {
        let tree = MimcTree::new(DEPTH);
        let index = 354184;
        criterion.bench_function("mimc_tree_proof", move |bencher| {
            bencher.iter(|| tree.proof(black_box(index)));
        });
    }

    fn bench_verify(criterion: &mut Criterion) {
        let tree = MimcTree::new(DEPTH);
        let index = 354184;
        let proof = tree.proof(index);
        let hash = [0_u8; 32];
        criterion.bench_function("mimc_verfiy", move |bencher| {
            bencher.iter(|| proof.root(black_box(hash)));
        });
    }
}
