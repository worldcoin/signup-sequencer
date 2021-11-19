use ethers::contract::abigen;

abigen!(
    Semaphore,
    r#"[
        event LeafInsertion(uint256 indexed leaf, uint256 indexed leafIndex)
        function insertIdentity(uint256 _identityCommitment) public onlyOwner returns (uint256)
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);
