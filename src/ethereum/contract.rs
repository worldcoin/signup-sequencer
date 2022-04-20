use ethers::contract::abigen;

abigen!(
    SemaphoreAirdrop,
    r#"[
        event MemberAdded(uint256 indexed groupId, uint256 identityCommitment, uint256 leaf)
        function getDepth(uint256 groupId) public view returns (uint8)
        function createAirdrop(uint256 airdropId, address coordinator, uint8 depth) public override
        function addRecipient(uint256 airdropId, uint256 identityCommitment) public override
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);
