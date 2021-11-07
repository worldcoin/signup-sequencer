use std::iter::{repeat, successors};

pub trait Leaf {
    type Hash: Clone + Eq;

    /// Hash value of an empty leaf
    fn empty_hash() -> Self::Hash;

    /// Compute the hash of a leaf
    fn hash_leaf(&self) -> Self::Hash;

    /// Compute the hash of an intermediate node
    fn hash_node(left: &Self::Hash, right: &Self::Hash) -> Self::Hash;
}

pub struct MerkleTree<T: Leaf> {
    /// Depth of the tree, # of layers including leaf layer
    depth: usize,

    /// Hash value of empty subtrees of given depth, starting at leaf level
    empty: Vec<T::Hash>,

    /// Hash values of tree nodes and leaves, breadth first order
    nodes: Vec<T::Hash>,
}

impl<T: Leaf> MerkleTree<T> {
    pub fn new(depth: usize) -> Self {
        // Compute empty node values, leaf to root
        let empty = successors(Some(T::empty_hash()), |prev| Some(T::hash_node(prev, prev)))
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

    pub fn root(&self) -> T::Hash {
        self.nodes[0].clone()
    }

    pub fn set(&mut self, leaf: usize, value: &T) {
        assert!(leaf < self.num_leaves());
        let hash = value.hash_leaf();

        // Update leaf
        let mut index = self.num_leaves() + leaf - 1;
        self.nodes[index] = hash;

        // Update tree nodes
        loop {
            // Map index to parent index
            index = ((index + 1) >> 1) - 1;

            // Recompute node hash
            let child = (index << 1) + 1; // Left child, right is +1
            self.nodes[index] = T::hash_node(&self.nodes[child], &self.nodes[child + 1]);

            // Stop if root
            if index == 0 {
                break;
            }
        }
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use ethers::utils::keccak256;
    use hex_literal::hex;
    use merkletree::merkle::Element;

    type Hash = [u8; 32];

    impl Leaf for Hash {
        type Hash = Hash;

        fn empty_hash() -> Self::Hash {
            Self::default()
        }

        fn hash_leaf(&self) -> Self::Hash {
            self.clone()
        }

        fn hash_node(left: &Self::Hash, right: &Self::Hash) -> Self::Hash {
            keccak256([left.clone(), right.clone()].concat())
        }
    }

    #[test]
    fn test_tree() {
        let mut tree = MerkleTree::<Hash>::new(3);
        assert_eq!(
            tree.root(),
            hex!("b4c11951957c6f8f642c4af61cd6b24640fec6dc7fc607ee8206a99e92410d30")
        );
        tree.set(
            0,
            &hex!("0000000000000000000000000000000000000000000000000000000000000001"),
        );
        assert_eq!(
            tree.root(),
            hex!("c1ba1812ff680ce84c1d5b4f1087eeb08147a4d510f3496b2849df3a73f5af95")
        );
        tree.set(
            1,
            &hex!("0000000000000000000000000000000000000000000000000000000000000002"),
        );
        assert_eq!(
            tree.root(),
            hex!("893760ec5b5bee236f29e85aef64f17139c3c1b7ff24ce64eb6315fca0f2485b")
        );
        tree.set(
            2,
            &hex!("0000000000000000000000000000000000000000000000000000000000000003"),
        );
        assert_eq!(
            tree.root(),
            hex!("222ff5e0b5877792c2bc1670e2ccd0c2c97cd7bb1672a57d598db05092d3d72c")
        );
        tree.set(
            3,
            &hex!("0000000000000000000000000000000000000000000000000000000000000004"),
        );
        assert_eq!(
            tree.root(),
            hex!("a9bb8c3f1f12e9aa903a50c47f314b57610a3ab32f2d463293f58836def38d36")
        );
    }
}
