/// Implements basic binary Merkle trees
///
/// # To do
///
/// * Batch set support
/// * Disk based storage backend (using mmaped files should be easy)
use std::{
    fmt::Debug,
    iter::{repeat, successors},
};

/// Hash types, values and algorithms for a Merkle tree
pub trait Hasher {
    /// Type of the leaf and node hashes
    type Hash: Clone + Eq;

    /// Hash value of an initial leaf
    fn initial_leaf() -> Self::Hash;

    /// Compute the hash of an intermediate node
    fn hash_node(left: &Self::Hash, right: &Self::Hash) -> Self::Hash;
}

/// Merkle tree with all leaf and intermediate hashes stored
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MerkleTree<H: Hasher> {
    /// Depth of the tree, # of layers including leaf layer
    depth: usize,

    /// Hash value of empty subtrees of given depth, starting at leaf level
    empty: Vec<H::Hash>,

    /// Hash values of tree nodes and leaves, breadth first order
    nodes: Vec<H::Hash>,
}

/// Element of a Merkle proof
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Branch<H: Hasher> {
    /// Left branch taken, value is the right sibling hash.
    Left(H::Hash),

    /// Right branch taken, value is the left sibling hash.
    Right(H::Hash),
}

/// Merkle proof path, bottom to top.
#[derive(Clone, PartialEq, Eq)]
pub struct Proof<H: Hasher>(Vec<Branch<H>>);

impl<H: Hasher> MerkleTree<H> {
    pub fn new(depth: usize) -> Self {
        // Compute empty node values, leaf to root
        let empty = successors(Some(H::initial_leaf()), |prev| {
            Some(H::hash_node(prev, prev))
        })
        .take(depth)
        .collect::<Vec<_>>();

        // Compute node values
        let nodes = empty
            .iter()
            .rev()
            .enumerate()
            .flat_map(|(depth, hash)| repeat(hash).take(1 << depth))
            .cloned()
            .collect::<Vec<_>>();
        debug_assert!(nodes.len() == (1 << depth) - 1);

        Self {
            depth,
            empty,
            nodes,
        }
    }

    pub fn num_leaves(&self) -> usize {
        self.depth
            .checked_sub(1)
            .map(|n| 1 << n)
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub fn root(&self) -> H::Hash {
        self.nodes[0].clone()
    }

    pub fn set(&mut self, leaf: usize, hash: H::Hash) {
        // Update leaf
        assert!(leaf < self.num_leaves());
        let mut index = self.num_leaves() + leaf - 1;
        self.nodes[index] = hash;

        // Update tree nodes
        while index != 0 {
            // Map index to parent index
            index = ((index + 1) >> 1) - 1;

            // Recompute node hash
            let child = (index << 1) + 1; // Left child, right is +1
            self.nodes[index] = H::hash_node(&self.nodes[child], &self.nodes[child + 1]);
        }
    }

    // TODO: Batch set
    // pub fn set_batch<I: IntoIterator<Item = H::Hash>>(&mut self, offset: usize,
    // hashes: I) {     todo!()
    // }

    pub fn proof(&self, leaf: usize) -> Proof<H> {
        let mut index = self.num_leaves() + leaf - 1;
        let mut path = Vec::with_capacity(self.depth);
        while index != 0 {
            // Add proof for node at index to parent
            path.push(match index & 1 {
                1 => Branch::Left(self.nodes[index + 1].clone()),
                0 => Branch::Right(self.nodes[index - 1].clone()),
                _ => unreachable!(),
            });

            // Map index to parent index
            index = ((index + 1) >> 1) - 1;
        }
        Proof(path)
    }

    #[allow(dead_code)]
    pub fn verify(&self, hash: H::Hash, proof: &Proof<H>) -> bool {
        proof.root(hash) == self.root()
    }
}

impl<H: Hasher> Proof<H> {
    /// Compute the leaf index for this proof
    #[allow(dead_code)]
    pub fn leaf_index(&self) -> usize {
        self.0.iter().rev().fold(0, |index, branch| {
            match branch {
                Branch::Left(_) => index << 1,
                Branch::Right(_) => (index << 1) + 1,
            }
        })
    }

    /// Compute the Merkle root given a leaf hash
    #[allow(dead_code)]
    pub fn root(&self, hash: H::Hash) -> H::Hash {
        self.0.iter().fold(hash, |hash, branch| {
            match branch {
                Branch::Left(sibbling) => H::hash_node(&hash, sibbling),
                Branch::Right(sibbling) => H::hash_node(sibbling, &hash),
            }
        })
    }
}

impl<H> Debug for Branch<H>
where
    H: Hasher,
    H::Hash: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Left(arg0) => f.debug_tuple("Left").field(arg0).finish(),
            Self::Right(arg0) => f.debug_tuple("Right").field(arg0).finish(),
        }
    }
}

impl<H> Debug for Proof<H>
where
    H: Hasher,
    H::Hash: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Proof").field(&self.0).finish()
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use ethers::utils::keccak256;
    use hex_literal::hex;

    struct Keccak;

    impl Hasher for Keccak {
        type Hash = [u8; 32];

        fn initial_leaf() -> Self::Hash {
            Self::Hash::default()
        }

        fn hash_node(left: &Self::Hash, right: &Self::Hash) -> Self::Hash {
            keccak256([*left, *right].concat())
        }
    }

    #[test]
    fn test_root() {
        let mut tree = MerkleTree::<Keccak>::new(3);
        assert_eq!(
            tree.root(),
            hex!("b4c11951957c6f8f642c4af61cd6b24640fec6dc7fc607ee8206a99e92410d30")
        );
        tree.set(
            0,
            hex!("0000000000000000000000000000000000000000000000000000000000000001"),
        );
        assert_eq!(
            tree.root(),
            hex!("c1ba1812ff680ce84c1d5b4f1087eeb08147a4d510f3496b2849df3a73f5af95")
        );
        tree.set(
            1,
            hex!("0000000000000000000000000000000000000000000000000000000000000002"),
        );
        assert_eq!(
            tree.root(),
            hex!("893760ec5b5bee236f29e85aef64f17139c3c1b7ff24ce64eb6315fca0f2485b")
        );
        tree.set(
            2,
            hex!("0000000000000000000000000000000000000000000000000000000000000003"),
        );
        assert_eq!(
            tree.root(),
            hex!("222ff5e0b5877792c2bc1670e2ccd0c2c97cd7bb1672a57d598db05092d3d72c")
        );
        tree.set(
            3,
            hex!("0000000000000000000000000000000000000000000000000000000000000004"),
        );
        assert_eq!(
            tree.root(),
            hex!("a9bb8c3f1f12e9aa903a50c47f314b57610a3ab32f2d463293f58836def38d36")
        );
    }

    #[test]
    fn test_proof() {
        let mut tree = MerkleTree::<Keccak>::new(3);
        tree.set(
            0,
            hex!("0000000000000000000000000000000000000000000000000000000000000001"),
        );
        tree.set(
            1,
            hex!("0000000000000000000000000000000000000000000000000000000000000002"),
        );
        tree.set(
            2,
            hex!("0000000000000000000000000000000000000000000000000000000000000003"),
        );
        tree.set(
            3,
            hex!("0000000000000000000000000000000000000000000000000000000000000004"),
        );

        let proof = tree.proof(2);
        assert_eq!(proof.leaf_index(), 2);
        assert!(tree.verify(
            hex!("0000000000000000000000000000000000000000000000000000000000000003"),
            &proof
        ));
        assert!(!tree.verify(
            hex!("0000000000000000000000000000000000000000000000000000000000000001"),
            &proof
        ));
    }
}
