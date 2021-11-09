pragma solidity ^0.6.12;
pragma experimental ABIEncoderV2;

import { Rollup } from "../hubble-contracts/contracts/rollup/Rollup.sol";
import { Semaphore } from "./Semaphore.sol";
import { Types } from "../hubble-contracts/contracts/libs/Types.sol";
import { MerkleTree } from "../hubble-contracts/contracts/libs/MerkleTree.sol";
import { Tx } from "../hubble-contracts/contracts/libs/Tx.sol";
// import "../node_modules/@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

contract WalletClaims is ERC20 {
    using Types for Types.Batch;
    using Types for Types.UserState;

    Semaphore public semaphore;
    address public immutable rollupAddress;

    // 50 Tokens, must be adjusted in wei and match encoding scheme
    uint256 public AIRDROP = 50;

    mapping(bytes32 => bool) public transfers;


    struct SenderPubKeys {
        address Sender;
        uint16 NumPubKeys;
    }

    mapping(uint256 => SenderPubKeys) public numPubKeysByBatch;

    uint232 public EXTERNAL_NULLIFIER = 123;

    constructor(Semaphore _semaphore, address _rollupAddress)
        ERC20("WalletClaims", "WC")
        public {
            rollupAddress = _rollupAddress;
            semaphore = _semaphore;
    }

    /*
     * 
     * @dev Gas cost 336_111
     */
    function commit(
        uint256[8] calldata proof,
        bytes calldata pubKeyHash,
        uint256 batchId,
        uint256 commitmentIdx,
        uint256 transferIdx,
        uint256 _root,
        uint256 _nullifierHash
    ) external {
        semaphore.broadcastSignal(
            pubKeyHash,
            proof,
            _root,
            _nullifierHash,
            EXTERNAL_NULLIFIER
        );
        transfers[
            keccak256(
                abi.encodePacked(
                    pubKeyHash,
                    batchId,
                    commitmentIdx,
                    transferIdx
                )
            )
        ] = true;

        if (numPubKeysByBatch[batchId].NumPubKeys != 0) {
            SenderPubKeys memory signerPubKeys = numPubKeysByBatch[batchId];
            require(signerPubKeys.Sender == msg.sender, "CA Address mismatch");
            numPubKeysByBatch[batchId] = SenderPubKeys(
                signerPubKeys.Sender,
                signerPubKeys.NumPubKeys + 1
            );
        } else {
            numPubKeysByBatch[batchId] = SenderPubKeys(msg.sender, 1);
        }
    }

    /*
     * @dev gas cost 83_931
     */
    function claimFunds(uint256 batchId) public {
        SenderPubKeys memory senderPubKeys = numPubKeysByBatch[batchId];

        require(senderPubKeys.NumPubKeys > 0, "NumPubKeys must be > 0");
        require(senderPubKeys.Sender != address(0), "Sender must not be null address");

        Rollup rollup = Rollup(rollupAddress);
        Types.Batch memory batch = rollup.getBatch(batchId);
        require(
            block.number >= batch.finaliseOn(),
            "WalletClaims.claimFunds: Batch shoould be finalised"
        );

        _mint(senderPubKeys.Sender, senderPubKeys.NumPubKeys * AIRDROP);
        // TODO any reentrancy concerns?
        numPubKeysByBatch[batchId].NumPubKeys = 0;
    }

    /*
     * @dev Gas cost 91_942
     */
    function fraudProofAmount(
        uint256 batchId,
        uint256 transferIdx,
        Types.TransferCommitmentInclusionProof memory commitmentProof
    ) public {
        Rollup rollup = Rollup(rollupAddress);
        Types.Batch memory batch = rollup.getBatch(batchId);
        require(
            checkInclusion(batch.commitmentRoot, commitmentProof),
            "Commitment is absent in the batch"
        );

        Tx.Create2Transfer memory _tx = Tx.create2TransferDecode(
            commitmentProof.commitment.body.txs,
            transferIdx
        );
        require(_tx.amount != AIRDROP, "Transfer amount is correct");
        // Fraud
        numPubKeysByBatch[batchId].NumPubKeys = 0;
        _mint(msg.sender, AIRDROP);
    }

    function fraudProofPubKeyCheck(
        bytes calldata pubKeyHash,
        uint256 batchId,
        uint256 commitmentIdx,
        uint256 transferIdx, 
        Types.TransferCommitmentInclusionProof memory commitmentProof,
        Types.SignatureProof memory stateProof
    ) view public returns (bool) {
        require(
            transfers[
                keccak256(
                    abi.encodePacked(
                        pubKeyHash,
                        batchId,
                        commitmentIdx,
                        transferIdx
                    )
                )
            ], "Transfer commitment not found."
        );
        Rollup rollup = Rollup(rollupAddress);
        Types.Batch memory batch = rollup.getBatch(batchId);
        require(
            checkInclusion(batch.commitmentRoot, commitmentProof),
            "Commitment is absent in the batch"
        );

        Tx.Create2Transfer memory _tx = Tx.create2TransferDecode(
            commitmentProof.commitment.body.txs,
            transferIdx
        );

        // Check that state leaf exists and that this leaf corresponds to the correct pubkey
        require(
            MerkleTree.verify(
                commitmentProof.commitment.stateRoot,
                keccak256(stateProof.states[0].encode()),
                _tx.toIndex,
                stateProof.stateWitnesses[0]
            ),
            "state inclusion proof invalid"
        );
        // check that pubkey is in account tree
        require(
            MerkleTree.verify(
                commitmentProof.commitment.body.accountRoot,
                keccak256(abi.encodePacked(stateProof.pubkeys[0])),
                stateProof.states[0].pubkeyID,
                stateProof.pubkeyWitnesses[0]
            ),
            "pubkey inclusion proof invalid"
        );
        // This verifies that the commitment made in `transfers` is to a different pubkeyhash
        bytes32 calculatedPubKeyHash = keccak256(abi.encodePacked(stateProof.pubkeys[0]));
        return keccak256(pubKeyHash) != keccak256(abi.encode(calculatedPubKeyHash));
    }

    /*
     * How to use: Sync the inputs from calldata for commit to produce fraud proof
     * @dev Gas cost 165_089
     */
    function fraudProofPubKey(
        bytes calldata pubKeyHash,
        uint256 batchId,
        uint256 commitmentIdx,
        uint256 transferIdx, 
        Types.TransferCommitmentInclusionProof memory commitmentProof,
        Types.SignatureProof memory stateProof
    ) public {
        require(
            transfers[
                keccak256(
                    abi.encodePacked(
                        pubKeyHash,
                        batchId,
                        commitmentIdx,
                        transferIdx
                    )
                )
            ], "Transfer commitment not found."
        );
        Rollup rollup = Rollup(rollupAddress);
        Types.Batch memory batch = rollup.getBatch(batchId);
        require(
            checkInclusion(batch.commitmentRoot, commitmentProof),
            "Commitment is absent in the batch"
        );

        Tx.Create2Transfer memory _tx = Tx.create2TransferDecode(
            commitmentProof.commitment.body.txs,
            transferIdx
        );

        // Check that state leaf exists and that this leaf corresponds to the correct pubkey
        require(
            MerkleTree.verify(
                commitmentProof.commitment.stateRoot,
                keccak256(stateProof.states[0].encode()),
                _tx.toIndex,
                stateProof.stateWitnesses[0]
            ),
            "state inclusion proof invalid"
        );
        // check that pubkey is in account tree
        require(
            MerkleTree.verify(
                commitmentProof.commitment.body.accountRoot,
                keccak256(abi.encodePacked(stateProof.pubkeys[0])),
                stateProof.states[0].pubkeyID,
                stateProof.pubkeyWitnesses[0]
            ),
            "pubkey inclusion proof invalid"
        );
        // This verifies that the commitment made in `transfers` is to a different pubkeyhash
        bytes memory calculatedPubKeyHash = abi.encodePacked(keccak256(abi.encodePacked(stateProof.pubkeys[0])));
        require(
            keccak256(pubKeyHash) != 
            keccak256(calculatedPubKeyHash),
            "committed pubKeyHash is correct"
        );
        numPubKeysByBatch[batchId].NumPubKeys = 0;
        _mint(msg.sender, AIRDROP);
    }

    function checkInclusion(
        bytes32 root,
        Types.TransferCommitmentInclusionProof memory proof
    ) internal pure returns (bool) {
        return
            MerkleTree.verify(
                root,
                Types.toHash(proof.commitment),
                proof.path,
                proof.witness
            );
    }
}
