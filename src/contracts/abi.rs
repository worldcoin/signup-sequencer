#![allow(clippy::extra_unused_lifetimes)]

use ethers::prelude::abigen;

abigen!(
    WorldId,
    r#"[
        struct RootInfo { uint256 root; uint128 supersededTimestamp; bool isValid }
        function registerIdentities(uint256[8] calldata insertionProof, uint256 preRoot, uint32 startIndex, uint256[] calldata identityCommitments, uint256 postRoot) public virtual
        function deleteIdentities(uint256[8] calldata deletionProof, uint32 batchSize, bytes calldata packedDeletionIndices, uint256 preRoot, uint256 postRoot) public virtual        
        function latestRoot() public view virtual returns (uint256 root)
        function owner() public view virtual returns (address)
        function queryRoot(uint256 root) public view virtual returns (RootInfo memory)
        function getRootHistoryExpiry() external view returns (uint256)
    ]"#,
);

abigen!(
    BridgedWorldId,
    r#"[
        function rootHistory(uint256 root) public view virtual returns (uint128 timestamp)
        function latestRoot() public view returns (uint256 root)
    ]"#
);
