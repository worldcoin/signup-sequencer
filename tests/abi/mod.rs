#![allow(clippy::extra_unused_lifetimes)]

use ethers::prelude::abigen;

abigen!(
    BatchingContract,
    r#"[
        struct RootInfo { uint256 root; uint128 supersededTimestamp; bool isValid }
        struct IdentityUpdate { uint32 leafIndex; uint256 oldCommitment; uint256 newCommitment; }
        function NO_SUCH_ROOT() public pure returns (RootInfo memory rootInfo)
        error UnreducedElement(uint8 elementType, uint256 element)
        error Unauthorized(address user)
        error InvalidCommitment(uint256 commitment)
        error ProofValidationFailure()
        error NotLatestRoot(uint256 providedRoot, uint256 latestRoot)
        error ExpiredRoot()
        error NonExistentRoot()
        error ImplementationNotInitalized()
        error NoSuchVerifier()
        constructor(address _logic, bytes memory data) payable
        function initialize(uint256 initialRoot, address _batchInsertionVerifiers, address _batchUpdateVerifiers, address _semaphoreVerifiers, bool _enableStateBridge, address initialStateBridgeProxyAddress) public virtual
        function registerIdentities(uint256[8] calldata insertionProof, uint256 preRoot, uint32 startIndex, uint256[] calldata identityCommitments, uint256 postRoot) public virtual
        function removeIdentities(uint256[8] calldata removalProof, uint256 preRoot, IdentityUpdate[] calldata removedIdentities, uint256 postRoot) public virtual
        function updateIdentities(uint256[8] calldata updateProof, uint256 preRoot, IdentityUpdate[] calldata updatedIdentities, uint256 postRoot) public virtual
        function calculateIdentityRegistrationInputHash(uint32 startIndex, uint256 preRoot, uint256 postRoot, uint256[] identityCommitments) public view virtual returns (bytes32 hash)
        function calculateIdentityUpdateInputHash(uint256 preRoot, uint256 postRoot, IdentityUpdate[] calldata identities) public view virtual returns (bytes32 hash)
        function latestRoot() public view virtual returns (uint256 root)
        function queryRoot(uint256 root) public view virtual returns (RootInfo memory rootInfo)
        function isInputInReducedForm(uint256 input) public view virtual returns (bool isInReducedForm)
        function checkValidRoot(uint256 root) public view virtual returns (bool)
        function getRegisterIdentitiesVerifierLookupTableAddress() public view virtual returns (address addr)
        function setRegisterIdentitiesVerifierLookupTable(address newVerifier) public virtual
        function getIdentityUpdateVerifierLookupTableAddress() public view virtual returns (address addr)
        function setIdentityUpdateVerifierLookupTable(address newVerifier) public virtual
        function getSemaphoreVerifierAddress() public view virtual returns (address addr)
        function setSemaphoreVerifier(address newVerifier) public virtual
        function getRootHistoryExpiry() public view virtual returns (uint256 expiryTime)
        function setRootHistoryExpiry(uint256 newExpiryTime) public virtual
        function verifyProof(uint256 root, uint256 signalHash, uint256 nullifierHash, uint256 externalNullifierHash, uint256[8] calldata proof) public view virtual
        function owner() public view virtual returns (address)
        function renounceOwnership() public virtual
        function transferOwnership(address newOwner) public virtual
        function proxiableUUID() external view virtual override returns (bytes32)
        function upgradeTo(address newImplementation) external virtual
        function upgradeToAndCall(address newImplementation, bytes memory data) external payable virtual
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);
