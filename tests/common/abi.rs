#![allow(clippy::extra_unused_lifetimes)]

use ethers::prelude::abigen;

abigen!(
    BatchingContract,
    r#"[
        struct RootInfo { uint256 root; uint128 supersededTimestamp; bool isValid }
        function initialize(uint8 treeDepth, uint256 initialRoot, address _batchInsertionVerifiers, address _batchUpdateVerifiers, address _semaphoreVerifier) public virtual
        function initializeV2(address _batchDeletionVerifiers) public virtual
        function verifyProof(uint256 root, uint256 signalHash, uint256 nullifierHash, uint256 externalNullifierHash, uint256[8] calldata proof) public view virtual
        function setRootHistoryExpiry(uint256 newExpiryTime) public virtual
        function queryRoot(uint256 root) public view virtual returns (RootInfo memory)
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);
