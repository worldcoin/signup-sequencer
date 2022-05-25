use ethers::contract::abigen;

abigen!(
    Semaphore,
    r#"[
        event MemberAdded(uint256 indexed groupId, uint256 identityCommitment, uint256 root)
        function manager() public view returns (address)
        function getDepth(uint256 groupId) public view returns (uint8)
        function createGroup(uint256 groupId, uint8 depth, uint256 zeroValue) public override
        function addMember(uint256 groupId, uint256 identityCommitment) public override
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);
