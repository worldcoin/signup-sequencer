#![allow(clippy::extra_unused_lifetimes)]

use alloy::sol;

sol! {
    #[sol(rpc)]
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    interface IWorldIDIdentityManager {
struct RootInfo { uint256 root; uint128 supersededTimestamp; bool isValid; }
function queryRoot(uint256 root) external view returns (RootInfo memory);
    }
}

pub use IWorldIDIdentityManager::RootInfo;
