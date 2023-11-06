#![allow(clippy::extra_unused_lifetimes)]

use ethers::prelude::abigen;

/// The `TreeChanged` event emitted by the `IdentityManager` contract.
/// Maps to the following enum in the contract code:
///
/// ```sol
/// enum TreeChange {
///     Insertion,
///     Deletion,
///     Update
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeChangeKind {
    Insertion,
    Deletion,
    Update,
}

impl From<u8> for TreeChangeKind {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Insertion,
            1 => Self::Deletion,
            2 => Self::Update,
            _ => panic!("Invalid value for TreeChangeKind: {}", value),
        }
    }
}

abigen!(
    WorldId,
    r#"[
        struct RootInfo { uint256 root; uint128 supersededTimestamp; bool isValid }
        event TreeChanged(uint256 indexed preRoot, uint8 indexed kind, uint256 indexed postRoot)
        function registerIdentities(uint256[8] calldata insertionProof, uint256 preRoot, uint32 startIndex, uint256[] calldata identityCommitments, uint256 postRoot) public virtual
        function deleteIdentities(uint256[8] calldata deletionProof, bytes calldata packedDeletionIndices, uint256 preRoot, uint256 postRoot) public virtual
        function latestRoot() public view virtual returns (uint256 root)
        function owner() public view virtual returns (address)
        function identityOperator() public view virtual returns (address)
        function queryRoot(uint256 root) public view virtual returns (RootInfo memory)
        function getRootHistoryExpiry() external view returns (uint256)
    ]"#,
);

abigen!(
    BridgedWorldId,
    r#"[
        event RootAdded(uint256 root, uint128 timestamp)
        function rootHistory(uint256 root) public view virtual returns (uint128 timestamp)
        function latestRoot() public view returns (uint256 root)
    ]"#
);
