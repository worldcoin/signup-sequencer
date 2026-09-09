#![allow(clippy::extra_unused_lifetimes)]

use alloy::sol;

sol! {
    #[sol(rpc)]
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    interface IWorldIDIdentityManager {
struct RootInfo { uint256 root; uint128 supersededTimestamp; bool isValid; }
function initialize(uint8 treeDepth, uint256 initialRoot, address _batchInsertionVerifiers, address _batchUpdateVerifiers, address _semaphoreVerifier) external;
function initializeV2(address _batchDeletionVerifiers) external;
function verifyProof(uint256 root, uint256 signalHash, uint256 nullifierHash, uint256 externalNullifierHash, uint256[8] calldata proof) external view;
function setRootHistoryExpiry(uint256 newExpiryTime) external;
function queryRoot(uint256 root) external view returns (RootInfo memory);
    }
}

pub use IWorldIDIdentityManager::RootInfo;
