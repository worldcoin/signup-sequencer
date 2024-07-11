#![allow(clippy::extra_unused_lifetimes)]

use ethers::prelude::abigen;

abigen!(
    IdentityManagerContract,
    r#"[
        struct RootInfo { uint256 root; uint128 supersededTimestamp; bool isValid }
        function queryRoot(uint256 root) public view virtual returns (RootInfo memory)
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);
