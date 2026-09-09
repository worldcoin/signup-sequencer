use alloy::sol;

/// The `TreeChanged` event is emitted by the `IdentityManager` contract.
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
            _ => panic!("Invalid value for TreeChangeKind: {value}"),
        }
    }
}

sol! {
    #[sol(rpc)]
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    interface WorldId {
        struct RootInfo { uint256 root; uint128 supersededTimestamp; bool isValid; }
        event TreeChanged(uint256 indexed preRoot, uint8 indexed kind, uint256 indexed postRoot);
        function registerIdentities(uint256[8] calldata insertionProof, uint256 preRoot, uint32 startIndex, uint256[] calldata identityCommitments, uint256 postRoot) external;
        function deleteIdentities(uint256[8] calldata deletionProof, bytes calldata packedDeletionIndices, uint256 preRoot, uint256 postRoot) external;
        function latestRoot() external view returns (uint256 root);
        function owner() external view returns (address);
        function identityOperator() external view returns (address);
        function queryRoot(uint256 root) external view returns (RootInfo memory);
        function getRootHistoryExpiry() external view returns (uint256);
    }
}

sol! {
    #[sol(rpc)]
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    interface BridgedWorldId {
        event RootAdded(uint256 root, uint128 timestamp);
        function rootHistory(uint256 root) external view returns (uint128 timestamp);
        function latestRoot() public view returns (uint256 root);
    }
}

pub use BridgedWorldId::RootAdded as RootAddedFilter;
pub use WorldId::TreeChanged as TreeChangedFilter;

#[cfg(test)]
mod tests {
    #[test]
    fn identity_contract_calldata_and_event_layout_are_unchanged() {
        use super::{TreeChangedFilter, WorldId};
        use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
        use alloy::sol_types::{SolCall, SolEvent};

        let call = WorldId::registerIdentitiesCall {
            insertionProof: [U256::ONE; 8],
            preRoot: U256::from(2),
            startIndex: 3,
            identityCommitments: vec![U256::from(4), U256::from(5)],
            postRoot: U256::from(6),
        };
        // Eight proof words plus pre-root, index, dynamic-array offset and post-root.
        let mut expected =
            keccak256("registerIdentities(uint256[8],uint256,uint32,uint256[],uint256)")[..4]
                .to_vec();
        for word in [1u64, 1, 1, 1, 1, 1, 1, 1, 2, 3, 384, 6, 2, 4, 5] {
            expected.extend_from_slice(&U256::from(word).to_be_bytes::<32>());
        }
        assert_eq!(call.abi_encode(), expected);

        let raw = alloy::primitives::Log::new(
            Address::ZERO,
            vec![
                TreeChangedFilter::SIGNATURE_HASH,
                B256::from(U256::from(2)),
                B256::ZERO,
                B256::from(U256::from(6)),
            ],
            Bytes::new(),
        )
        .unwrap();
        let log = alloy::rpc::types::Log {
            inner: raw,
            ..Default::default()
        };
        let event = log.log_decode::<TreeChangedFilter>().unwrap().inner.data;
        assert_eq!(event.preRoot, U256::from(2));
        assert_eq!(event.kind, 0);
        assert_eq!(event.postRoot, U256::from(6));
    }
}
