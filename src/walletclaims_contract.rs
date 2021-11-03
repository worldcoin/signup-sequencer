pub use walletclaims_mod::*;
#[allow(clippy::too_many_arguments)]
mod walletclaims_mod {
    #![allow(clippy::enum_variant_names)]
    #![allow(dead_code)]
    #![allow(clippy::type_complexity)]
    #![allow(unused_imports)]
    use ethers::{
        contract::{
            builders::{ContractCall, Event},
            Contract, Lazy,
        },
        core::{
            abi::{Abi, Detokenize, InvalidOutputType, Token, Tokenizable},
            types::*,
        },
        providers::Middleware,
    };
    /// WalletClaims was auto-generated with ethers-rs Abigen. More information at: https://github.com/gakonst/ethers-rs
    use std::sync::Arc;
    pub static WALLETCLAIMS_ABI: ethers::contract::Lazy<ethers::core::abi::Abi> =
        ethers::contract::Lazy::new(|| {
            serde_json::from_str(
                "[{\"inputs\":[{\"internalType\":\"contract \
                 Semaphore\",\"name\":\"_semaphore\",\"type\":\"address\"},{\"internalType\":\"\
                 address\",\"name\":\"_rollupAddress\",\"type\":\"address\"}],\"stateMutability\":\
                 \"nonpayable\",\"type\":\"constructor\"},{\"anonymous\":false,\"inputs\":[{\"\
                 indexed\":true,\"internalType\":\"address\",\"name\":\"owner\",\"type\":\"\
                 address\"},{\"indexed\":true,\"internalType\":\"address\",\"name\":\"spender\",\"\
                 type\":\"address\"},{\"indexed\":false,\"internalType\":\"uint256\",\"name\":\"\
                 value\",\"type\":\"uint256\"}],\"name\":\"Approval\",\"type\":\"event\"},{\"\
                 anonymous\":false,\"inputs\":[{\"indexed\":true,\"internalType\":\"address\",\"\
                 name\":\"from\",\"type\":\"address\"},{\"indexed\":true,\"internalType\":\"\
                 address\",\"name\":\"to\",\"type\":\"address\"},{\"indexed\":false,\"\
                 internalType\":\"uint256\",\"name\":\"value\",\"type\":\"uint256\"}],\"name\":\"\
                 Transfer\",\"type\":\"event\"},{\"inputs\":[],\"name\":\"AIRDROP\",\"outputs\":\
                 [{\"internalType\":\"uint256\",\"name\":\"\",\"type\":\"uint256\"}],\"\
                 stateMutability\":\"view\",\"type\":\"function\"},{\"inputs\":[],\"name\":\"\
                 EXTERNAL_NULLIFIER\",\"outputs\":[{\"internalType\":\"uint232\",\"name\":\"\",\"\
                 type\":\"uint232\"}],\"stateMutability\":\"view\",\"type\":\"function\"},{\"\
                 inputs\":[{\"internalType\":\"address\",\"name\":\"owner\",\"type\":\"address\"},\
                 {\"internalType\":\"address\",\"name\":\"spender\",\"type\":\"address\"}],\"name\
                 \":\"allowance\",\"outputs\":[{\"internalType\":\"uint256\",\"name\":\"\",\"type\"\
                 :\"uint256\"}],\"stateMutability\":\"view\",\"type\":\"function\"},{\"inputs\":\
                 [{\"internalType\":\"address\",\"name\":\"spender\",\"type\":\"address\"},{\"\
                 internalType\":\"uint256\",\"name\":\"amount\",\"type\":\"uint256\"}],\"name\":\"\
                 approve\",\"outputs\":[{\"internalType\":\"bool\",\"name\":\"\",\"type\":\"bool\"\
                 }],\"stateMutability\":\"nonpayable\",\"type\":\"function\"},{\"inputs\":[{\"\
                 internalType\":\"address\",\"name\":\"account\",\"type\":\"address\"}],\"name\":\
                 \"balanceOf\",\"outputs\":[{\"internalType\":\"uint256\",\"name\":\"\",\"type\":\"\
                 uint256\"}],\"stateMutability\":\"view\",\"type\":\"function\"},{\"inputs\":[{\"\
                 internalType\":\"uint256\",\"name\":\"batchId\",\"type\":\"uint256\"}],\"name\":\
                 \"claimFunds\",\"outputs\":[],\"stateMutability\":\"nonpayable\",\"type\":\"\
                 function\"},{\"inputs\":[{\"internalType\":\"uint256[8]\",\"name\":\"proof\",\"\
                 type\":\"uint256[8]\"},{\"internalType\":\"bytes\",\"name\":\"pubKeyHash\",\"\
                 type\":\"bytes\"},{\"internalType\":\"uint256\",\"name\":\"batchId\",\"type\":\"\
                 uint256\"},{\"internalType\":\"uint256\",\"name\":\"commitmentIdx\",\"type\":\"\
                 uint256\"},{\"internalType\":\"uint256\",\"name\":\"transferIdx\",\"type\":\"\
                 uint256\"},{\"internalType\":\"uint256\",\"name\":\"_root\",\"type\":\"uint256\"\
                 },{\"internalType\":\"uint256\",\"name\":\"_nullifierHash\",\"type\":\"uint256\"\
                 }],\"name\":\"commit\",\"outputs\":[],\"stateMutability\":\"nonpayable\",\"type\"\
                 :\"function\"},{\"inputs\":[],\"name\":\"decimals\",\"outputs\":[{\"internalType\
                 \":\"uint8\",\"name\":\"\",\"type\":\"uint8\"}],\"stateMutability\":\"view\",\"\
                 type\":\"function\"},{\"inputs\":[{\"internalType\":\"address\",\"name\":\"\
                 spender\",\"type\":\"address\"},{\"internalType\":\"uint256\",\"name\":\"\
                 subtractedValue\",\"type\":\"uint256\"}],\"name\":\"decreaseAllowance\",\"\
                 outputs\":[{\"internalType\":\"bool\",\"name\":\"\",\"type\":\"bool\"}],\"\
                 stateMutability\":\"nonpayable\",\"type\":\"function\"},{\"inputs\":[{\"\
                 internalType\":\"uint256\",\"name\":\"batchId\",\"type\":\"uint256\"},{\"\
                 internalType\":\"uint256\",\"name\":\"transferIdx\",\"type\":\"uint256\"},{\"\
                 components\":[{\"components\":[{\"internalType\":\"bytes32\",\"name\":\"\
                 stateRoot\",\"type\":\"bytes32\"},{\"components\":[{\"internalType\":\"bytes32\",\
                 \"name\":\"accountRoot\",\"type\":\"bytes32\"},{\"internalType\":\"uint256[2]\",\
                 \"name\":\"signature\",\"type\":\"uint256[2]\"},{\"internalType\":\"uint256\",\"\
                 name\":\"feeReceiver\",\"type\":\"uint256\"},{\"internalType\":\"bytes\",\"name\"\
                 :\"txs\",\"type\":\"bytes\"}],\"internalType\":\"struct \
                 Types.TransferBody\",\"name\":\"body\",\"type\":\"tuple\"}],\"internalType\":\"\
                 struct Types.TransferCommitment\",\"name\":\"commitment\",\"type\":\"tuple\"},{\"\
                 internalType\":\"uint256\",\"name\":\"path\",\"type\":\"uint256\"},{\"\
                 internalType\":\"bytes32[]\",\"name\":\"witness\",\"type\":\"bytes32[]\"}],\"\
                 internalType\":\"struct \
                 Types.TransferCommitmentInclusionProof\",\"name\":\"commitmentProof\",\"type\":\"\
                 tuple\"}],\"name\":\"fraudProofAmount\",\"outputs\":[],\"stateMutability\":\"\
                 nonpayable\",\"type\":\"function\"},{\"inputs\":[{\"internalType\":\"bytes\",\"\
                 name\":\"pubKeyHash\",\"type\":\"bytes\"},{\"internalType\":\"uint256\",\"name\":\
                 \"batchId\",\"type\":\"uint256\"},{\"internalType\":\"uint256\",\"name\":\"\
                 commitmentIdx\",\"type\":\"uint256\"},{\"internalType\":\"uint256\",\"name\":\"\
                 transferIdx\",\"type\":\"uint256\"},{\"components\":[{\"components\":[{\"\
                 internalType\":\"bytes32\",\"name\":\"stateRoot\",\"type\":\"bytes32\"},{\"\
                 components\":[{\"internalType\":\"bytes32\",\"name\":\"accountRoot\",\"type\":\"\
                 bytes32\"},{\"internalType\":\"uint256[2]\",\"name\":\"signature\",\"type\":\"\
                 uint256[2]\"},{\"internalType\":\"uint256\",\"name\":\"feeReceiver\",\"type\":\"\
                 uint256\"},{\"internalType\":\"bytes\",\"name\":\"txs\",\"type\":\"bytes\"}],\"\
                 internalType\":\"struct \
                 Types.TransferBody\",\"name\":\"body\",\"type\":\"tuple\"}],\"internalType\":\"\
                 struct Types.TransferCommitment\",\"name\":\"commitment\",\"type\":\"tuple\"},{\"\
                 internalType\":\"uint256\",\"name\":\"path\",\"type\":\"uint256\"},{\"\
                 internalType\":\"bytes32[]\",\"name\":\"witness\",\"type\":\"bytes32[]\"}],\"\
                 internalType\":\"struct \
                 Types.TransferCommitmentInclusionProof\",\"name\":\"commitmentProof\",\"type\":\"\
                 tuple\"},{\"components\":[{\"components\":[{\"internalType\":\"uint256\",\"name\"\
                 :\"pubkeyID\",\"type\":\"uint256\"},{\"internalType\":\"uint256\",\"name\":\"\
                 tokenID\",\"type\":\"uint256\"},{\"internalType\":\"uint256\",\"name\":\"balance\
                 \",\"type\":\"uint256\"},{\"internalType\":\"uint256\",\"name\":\"nonce\",\"type\"\
                 :\"uint256\"}],\"internalType\":\"struct \
                 Types.UserState[]\",\"name\":\"states\",\"type\":\"tuple[]\"},{\"internalType\":\
                 \"bytes32[][]\",\"name\":\"stateWitnesses\",\"type\":\"bytes32[][]\"},{\"\
                 internalType\":\"uint256[4][]\",\"name\":\"pubkeys\",\"type\":\"uint256[4][]\"},\
                 {\"internalType\":\"bytes32[][]\",\"name\":\"pubkeyWitnesses\",\"type\":\"\
                 bytes32[][]\"}],\"internalType\":\"struct \
                 Types.SignatureProof\",\"name\":\"stateProof\",\"type\":\"tuple\"}],\"name\":\"\
                 fraudProofPubKey\",\"outputs\":[],\"stateMutability\":\"nonpayable\",\"type\":\"\
                 function\"},{\"inputs\":[{\"internalType\":\"bytes\",\"name\":\"pubKeyHash\",\"\
                 type\":\"bytes\"},{\"internalType\":\"uint256\",\"name\":\"batchId\",\"type\":\"\
                 uint256\"},{\"internalType\":\"uint256\",\"name\":\"commitmentIdx\",\"type\":\"\
                 uint256\"},{\"internalType\":\"uint256\",\"name\":\"transferIdx\",\"type\":\"\
                 uint256\"},{\"components\":[{\"components\":[{\"internalType\":\"bytes32\",\"\
                 name\":\"stateRoot\",\"type\":\"bytes32\"},{\"components\":[{\"internalType\":\"\
                 bytes32\",\"name\":\"accountRoot\",\"type\":\"bytes32\"},{\"internalType\":\"\
                 uint256[2]\",\"name\":\"signature\",\"type\":\"uint256[2]\"},{\"internalType\":\"\
                 uint256\",\"name\":\"feeReceiver\",\"type\":\"uint256\"},{\"internalType\":\"\
                 bytes\",\"name\":\"txs\",\"type\":\"bytes\"}],\"internalType\":\"struct \
                 Types.TransferBody\",\"name\":\"body\",\"type\":\"tuple\"}],\"internalType\":\"\
                 struct Types.TransferCommitment\",\"name\":\"commitment\",\"type\":\"tuple\"},{\"\
                 internalType\":\"uint256\",\"name\":\"path\",\"type\":\"uint256\"},{\"\
                 internalType\":\"bytes32[]\",\"name\":\"witness\",\"type\":\"bytes32[]\"}],\"\
                 internalType\":\"struct \
                 Types.TransferCommitmentInclusionProof\",\"name\":\"commitmentProof\",\"type\":\"\
                 tuple\"},{\"components\":[{\"components\":[{\"internalType\":\"uint256\",\"name\"\
                 :\"pubkeyID\",\"type\":\"uint256\"},{\"internalType\":\"uint256\",\"name\":\"\
                 tokenID\",\"type\":\"uint256\"},{\"internalType\":\"uint256\",\"name\":\"balance\
                 \",\"type\":\"uint256\"},{\"internalType\":\"uint256\",\"name\":\"nonce\",\"type\"\
                 :\"uint256\"}],\"internalType\":\"struct \
                 Types.UserState[]\",\"name\":\"states\",\"type\":\"tuple[]\"},{\"internalType\":\
                 \"bytes32[][]\",\"name\":\"stateWitnesses\",\"type\":\"bytes32[][]\"},{\"\
                 internalType\":\"uint256[4][]\",\"name\":\"pubkeys\",\"type\":\"uint256[4][]\"},\
                 {\"internalType\":\"bytes32[][]\",\"name\":\"pubkeyWitnesses\",\"type\":\"\
                 bytes32[][]\"}],\"internalType\":\"struct \
                 Types.SignatureProof\",\"name\":\"stateProof\",\"type\":\"tuple\"}],\"name\":\"\
                 fraudProofPubKeyCheck\",\"outputs\":[{\"internalType\":\"bool\",\"name\":\"\",\"\
                 type\":\"bool\"}],\"stateMutability\":\"view\",\"type\":\"function\"},{\"inputs\"\
                 :[{\"internalType\":\"address\",\"name\":\"spender\",\"type\":\"address\"},{\"\
                 internalType\":\"uint256\",\"name\":\"addedValue\",\"type\":\"uint256\"}],\"name\
                 \":\"increaseAllowance\",\"outputs\":[{\"internalType\":\"bool\",\"name\":\"\",\"\
                 type\":\"bool\"}],\"stateMutability\":\"nonpayable\",\"type\":\"function\"},{\"\
                 inputs\":[],\"name\":\"name\",\"outputs\":[{\"internalType\":\"string\",\"name\":\
                 \"\",\"type\":\"string\"}],\"stateMutability\":\"view\",\"type\":\"function\"},{\
                 \"inputs\":[{\"internalType\":\"uint256\",\"name\":\"\",\"type\":\"uint256\"}],\"\
                 name\":\"numPubKeysByBatch\",\"outputs\":[{\"internalType\":\"address\",\"name\":\
                 \"Sender\",\"type\":\"address\"},{\"internalType\":\"uint16\",\"name\":\"\
                 NumPubKeys\",\"type\":\"uint16\"}],\"stateMutability\":\"view\",\"type\":\"\
                 function\"},{\"inputs\":[],\"name\":\"rollupAddress\",\"outputs\":[{\"\
                 internalType\":\"address\",\"name\":\"\",\"type\":\"address\"}],\"\
                 stateMutability\":\"view\",\"type\":\"function\"},{\"inputs\":[],\"name\":\"\
                 semaphore\",\"outputs\":[{\"internalType\":\"contract \
                 Semaphore\",\"name\":\"\",\"type\":\"address\"}],\"stateMutability\":\"view\",\"\
                 type\":\"function\"},{\"inputs\":[],\"name\":\"symbol\",\"outputs\":[{\"\
                 internalType\":\"string\",\"name\":\"\",\"type\":\"string\"}],\"stateMutability\"\
                 :\"view\",\"type\":\"function\"},{\"inputs\":[],\"name\":\"totalSupply\",\"\
                 outputs\":[{\"internalType\":\"uint256\",\"name\":\"\",\"type\":\"uint256\"}],\"\
                 stateMutability\":\"view\",\"type\":\"function\"},{\"inputs\":[{\"internalType\":\
                 \"address\",\"name\":\"recipient\",\"type\":\"address\"},{\"internalType\":\"\
                 uint256\",\"name\":\"amount\",\"type\":\"uint256\"}],\"name\":\"transfer\",\"\
                 outputs\":[{\"internalType\":\"bool\",\"name\":\"\",\"type\":\"bool\"}],\"\
                 stateMutability\":\"nonpayable\",\"type\":\"function\"},{\"inputs\":[{\"\
                 internalType\":\"address\",\"name\":\"sender\",\"type\":\"address\"},{\"\
                 internalType\":\"address\",\"name\":\"recipient\",\"type\":\"address\"},{\"\
                 internalType\":\"uint256\",\"name\":\"amount\",\"type\":\"uint256\"}],\"name\":\"\
                 transferFrom\",\"outputs\":[{\"internalType\":\"bool\",\"name\":\"\",\"type\":\"\
                 bool\"}],\"stateMutability\":\"nonpayable\",\"type\":\"function\"},{\"inputs\":\
                 [{\"internalType\":\"bytes32\",\"name\":\"\",\"type\":\"bytes32\"}],\"name\":\"\
                 transfers\",\"outputs\":[{\"internalType\":\"bool\",\"name\":\"\",\"type\":\"\
                 bool\"}],\"stateMutability\":\"view\",\"type\":\"function\"}]",
            )
            .expect("invalid abi")
        });
    #[derive(Clone)]
    pub struct WalletClaims<M>(ethers::contract::Contract<M>);
    impl<M> std::ops::Deref for WalletClaims<M> {
        type Target = ethers::contract::Contract<M>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl<M: ethers::providers::Middleware> std::fmt::Debug for WalletClaims<M> {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.debug_tuple(stringify!(WalletClaims))
                .field(&self.address())
                .finish()
        }
    }
    impl<'a, M: ethers::providers::Middleware> WalletClaims<M> {
        /// Creates a new contract instance with the specified `ethers`
        /// client at the given `Address`. The contract derefs to a
        /// `ethers::Contract`
        /// object
        pub fn new<T: Into<ethers::core::types::Address>>(
            address: T,
            client: ::std::sync::Arc<M>,
        ) -> Self {
            let contract =
                ethers::contract::Contract::new(address.into(), WALLETCLAIMS_ABI.clone(), client);
            Self(contract)
        }

        /// Calls the contract's `AIRDROP` (0x1fa0d621) function
        pub fn airdrop(
            &self,
        ) -> ethers::contract::builders::ContractCall<M, ethers::core::types::U256> {
            self.0
                .method_hash([31, 160, 214, 33], ())
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `EXTERNAL_NULLIFIER` (0x57b33276) function
        pub fn external_nullifier(
            &self,
        ) -> ethers::contract::builders::ContractCall<M, ethers::core::types::U256> {
            self.0
                .method_hash([87, 179, 50, 118], ())
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `allowance` (0xdd62ed3e) function
        pub fn allowance(
            &self,
            owner: ethers::core::types::Address,
            spender: ethers::core::types::Address,
        ) -> ethers::contract::builders::ContractCall<M, ethers::core::types::U256> {
            self.0
                .method_hash([221, 98, 237, 62], (owner, spender))
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `approve` (0x095ea7b3) function
        pub fn approve(
            &self,
            spender: ethers::core::types::Address,
            amount: ethers::core::types::U256,
        ) -> ethers::contract::builders::ContractCall<M, bool> {
            self.0
                .method_hash([9, 94, 167, 179], (spender, amount))
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `balanceOf` (0x70a08231) function
        pub fn balance_of(
            &self,
            account: ethers::core::types::Address,
        ) -> ethers::contract::builders::ContractCall<M, ethers::core::types::U256> {
            self.0
                .method_hash([112, 160, 130, 49], account)
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `claimFunds` (0x1b55e338) function
        pub fn claim_funds(
            &self,
            batch_id: ethers::core::types::U256,
        ) -> ethers::contract::builders::ContractCall<M, ()> {
            self.0
                .method_hash([27, 85, 227, 56], batch_id)
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `commit` (0x30517917) function
        pub fn commit(
            &self,
            proof: [ethers::core::types::U256; 8usize],
            pub_key_hash: Vec<u8>,
            batch_id: ethers::core::types::U256,
            commitment_idx: ethers::core::types::U256,
            transfer_idx: ethers::core::types::U256,
            root: ethers::core::types::U256,
            nullifier_hash: ethers::core::types::U256,
        ) -> ethers::contract::builders::ContractCall<M, ()> {
            self.0
                .method_hash(
                    [48, 81, 121, 23],
                    (
                        proof,
                        pub_key_hash,
                        batch_id,
                        commitment_idx,
                        transfer_idx,
                        root,
                        nullifier_hash,
                    ),
                )
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `decimals` (0x313ce567) function
        pub fn decimals(&self) -> ethers::contract::builders::ContractCall<M, u8> {
            self.0
                .method_hash([49, 60, 229, 103], ())
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `decreaseAllowance` (0xa457c2d7) function
        pub fn decrease_allowance(
            &self,
            spender: ethers::core::types::Address,
            subtracted_value: ethers::core::types::U256,
        ) -> ethers::contract::builders::ContractCall<M, bool> {
            self.0
                .method_hash([164, 87, 194, 215], (spender, subtracted_value))
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `fraudProofAmount` (0x66be976e) function
        pub fn fraud_proof_amount(
            &self,
            batch_id: ethers::core::types::U256,
            transfer_idx: ethers::core::types::U256,
            commitment_proof: TransferCommitmentInclusionProof,
        ) -> ethers::contract::builders::ContractCall<M, ()> {
            self.0
                .method_hash(
                    [102, 190, 151, 110],
                    (batch_id, transfer_idx, commitment_proof),
                )
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `fraudProofPubKey` (0x48abc71e) function
        pub fn fraud_proof_pub_key(
            &self,
            pub_key_hash: Vec<u8>,
            batch_id: ethers::core::types::U256,
            commitment_idx: ethers::core::types::U256,
            transfer_idx: ethers::core::types::U256,
            commitment_proof: TransferCommitmentInclusionProof,
            state_proof: SignatureProof,
        ) -> ethers::contract::builders::ContractCall<M, ()> {
            self.0
                .method_hash(
                    [72, 171, 199, 30],
                    (
                        pub_key_hash,
                        batch_id,
                        commitment_idx,
                        transfer_idx,
                        commitment_proof,
                        state_proof,
                    ),
                )
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `fraudProofPubKeyCheck` (0x8a21667e) function
        pub fn fraud_proof_pub_key_check(
            &self,
            pub_key_hash: Vec<u8>,
            batch_id: ethers::core::types::U256,
            commitment_idx: ethers::core::types::U256,
            transfer_idx: ethers::core::types::U256,
            commitment_proof: TransferCommitmentInclusionProof,
            state_proof: SignatureProof,
        ) -> ethers::contract::builders::ContractCall<M, bool> {
            self.0
                .method_hash(
                    [138, 33, 102, 126],
                    (
                        pub_key_hash,
                        batch_id,
                        commitment_idx,
                        transfer_idx,
                        commitment_proof,
                        state_proof,
                    ),
                )
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `increaseAllowance` (0x39509351) function
        pub fn increase_allowance(
            &self,
            spender: ethers::core::types::Address,
            added_value: ethers::core::types::U256,
        ) -> ethers::contract::builders::ContractCall<M, bool> {
            self.0
                .method_hash([57, 80, 147, 81], (spender, added_value))
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `name` (0x06fdde03) function
        pub fn name(&self) -> ethers::contract::builders::ContractCall<M, String> {
            self.0
                .method_hash([6, 253, 222, 3], ())
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `numPubKeysByBatch` (0xb6d34146) function
        pub fn num_pub_keys_by_batch(
            &self,
            p0: ethers::core::types::U256,
        ) -> ethers::contract::builders::ContractCall<M, (ethers::core::types::Address, u16)>
        {
            self.0
                .method_hash([182, 211, 65, 70], p0)
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `rollupAddress` (0x5ec6a8df) function
        pub fn rollup_address(
            &self,
        ) -> ethers::contract::builders::ContractCall<M, ethers::core::types::Address> {
            self.0
                .method_hash([94, 198, 168, 223], ())
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `semaphore` (0x7b5d2534) function
        pub fn semaphore(
            &self,
        ) -> ethers::contract::builders::ContractCall<M, ethers::core::types::Address> {
            self.0
                .method_hash([123, 93, 37, 52], ())
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `symbol` (0x95d89b41) function
        pub fn symbol(&self) -> ethers::contract::builders::ContractCall<M, String> {
            self.0
                .method_hash([149, 216, 155, 65], ())
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `totalSupply` (0x18160ddd) function
        pub fn total_supply(
            &self,
        ) -> ethers::contract::builders::ContractCall<M, ethers::core::types::U256> {
            self.0
                .method_hash([24, 22, 13, 221], ())
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `transfer` (0xa9059cbb) function
        pub fn transfer(
            &self,
            recipient: ethers::core::types::Address,
            amount: ethers::core::types::U256,
        ) -> ethers::contract::builders::ContractCall<M, bool> {
            self.0
                .method_hash([169, 5, 156, 187], (recipient, amount))
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `transferFrom` (0x23b872dd) function
        pub fn transfer_from(
            &self,
            sender: ethers::core::types::Address,
            recipient: ethers::core::types::Address,
            amount: ethers::core::types::U256,
        ) -> ethers::contract::builders::ContractCall<M, bool> {
            self.0
                .method_hash([35, 184, 114, 221], (sender, recipient, amount))
                .expect("method not found (this should never happen)")
        }

        /// Calls the contract's `transfers` (0x3c64f04b) function
        pub fn transfers(&self, p0: [u8; 32]) -> ethers::contract::builders::ContractCall<M, bool> {
            self.0
                .method_hash([60, 100, 240, 75], p0)
                .expect("method not found (this should never happen)")
        }

        /// Gets the contract's `Approval` event
        pub fn approval_filter(&self) -> ethers::contract::builders::Event<M, ApprovalFilter> {
            self.0.event()
        }

        /// Gets the contract's `Transfer` event
        pub fn transfer_filter(&self) -> ethers::contract::builders::Event<M, TransferFilter> {
            self.0.event()
        }

        /// Returns an [`Event`](#ethers_contract::builders::Event) builder for
        /// all events of this contract
        pub fn events(&self) -> ethers::contract::builders::Event<M, WalletClaimsEvents> {
            self.0.event_with_filter(Default::default())
        }
    }
    #[derive(Clone, Debug, Default, Eq, PartialEq, ethers :: contract :: EthEvent)]
    #[ethevent(name = "Approval", abi = "Approval(address,address,uint256)")]
    pub struct ApprovalFilter {
        #[ethevent(indexed)]
        pub owner:   ethers::core::types::Address,
        #[ethevent(indexed)]
        pub spender: ethers::core::types::Address,
        pub value:   ethers::core::types::U256,
    }
    #[derive(Clone, Debug, Default, Eq, PartialEq, ethers :: contract :: EthEvent)]
    #[ethevent(name = "Transfer", abi = "Transfer(address,address,uint256)")]
    pub struct TransferFilter {
        #[ethevent(indexed)]
        pub from:  ethers::core::types::Address,
        #[ethevent(indexed)]
        pub to:    ethers::core::types::Address,
        pub value: ethers::core::types::U256,
    }
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WalletClaimsEvents {
        ApprovalFilter(ApprovalFilter),
        TransferFilter(TransferFilter),
    }
    impl ethers::core::abi::Tokenizable for WalletClaimsEvents {
        fn from_token(
            token: ethers::core::abi::Token,
        ) -> Result<Self, ethers::core::abi::InvalidOutputType>
        where
            Self: Sized,
        {
            if let Ok(decoded) = ApprovalFilter::from_token(token.clone()) {
                return Ok(WalletClaimsEvents::ApprovalFilter(decoded));
            }
            if let Ok(decoded) = TransferFilter::from_token(token.clone()) {
                return Ok(WalletClaimsEvents::TransferFilter(decoded));
            }
            Err(ethers::core::abi::InvalidOutputType(
                "Failed to decode all event variants".to_string(),
            ))
        }

        fn into_token(self) -> ethers::core::abi::Token {
            match self {
                WalletClaimsEvents::ApprovalFilter(element) => element.into_token(),
                WalletClaimsEvents::TransferFilter(element) => element.into_token(),
            }
        }
    }
    impl ethers::core::abi::TokenizableItem for WalletClaimsEvents {}
    impl ethers::contract::EthLogDecode for WalletClaimsEvents {
        fn decode_log(log: &ethers::core::abi::RawLog) -> Result<Self, ethers::core::abi::Error>
        where
            Self: Sized,
        {
            if let Ok(decoded) = ApprovalFilter::decode_log(log) {
                return Ok(WalletClaimsEvents::ApprovalFilter(decoded));
            }
            if let Ok(decoded) = TransferFilter::decode_log(log) {
                return Ok(WalletClaimsEvents::TransferFilter(decoded));
            }
            Err(ethers::core::abi::Error::InvalidData)
        }
    }
    /// `SignatureProof((uint256,uint256,uint256,uint256)[],bytes32[][],
    /// uint256[4][],bytes32[][])`
    #[derive(Clone, Debug, Default, Eq, PartialEq, ethers :: contract :: EthAbiType)]
    pub struct SignatureProof {
        pub states:           ::std::vec::Vec<UserState>,
        pub state_witnesses:  Vec<Vec<[u8; 32]>>,
        pub pubkeys:          Vec<[ethers::core::types::U256; 4]>,
        pub pubkey_witnesses: Vec<Vec<[u8; 32]>>,
    }
    /// `TransferBody(bytes32,uint256[2],uint256,bytes)`
    #[derive(Clone, Debug, Default, Eq, PartialEq, ethers :: contract :: EthAbiType)]
    pub struct TransferBody {
        pub account_root: [u8; 32],
        pub signature:    [ethers::core::types::U256; 2],
        pub fee_receiver: ethers::core::types::U256,
        pub txs:          Vec<u8>,
    }
    /// `TransferCommitment(bytes32,(bytes32,uint256[2],uint256,bytes))`
    #[derive(Clone, Debug, Default, Eq, PartialEq, ethers :: contract :: EthAbiType)]
    pub struct TransferCommitment {
        pub state_root: [u8; 32],
        pub body:       TransferBody,
    }
    /// `TransferCommitmentInclusionProof((bytes32,(bytes32,uint256[2],uint256,
    /// bytes)),uint256,bytes32[])`
    #[derive(Clone, Debug, Default, Eq, PartialEq, ethers :: contract :: EthAbiType)]
    pub struct TransferCommitmentInclusionProof {
        pub commitment: TransferCommitment,
        pub path:       ethers::core::types::U256,
        pub witness:    Vec<[u8; 32]>,
    }
    /// `UserState(uint256,uint256,uint256,uint256)`
    #[derive(Clone, Debug, Default, Eq, PartialEq, ethers :: contract :: EthAbiType)]
    pub struct UserState {
        pub pubkey_id: ethers::core::types::U256,
        pub token_id:  ethers::core::types::U256,
        pub balance:   ethers::core::types::U256,
        pub nonce:     ethers::core::types::U256,
    }
}
