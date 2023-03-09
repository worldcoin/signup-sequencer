use crate::contracts::abi::RegisterIdentitiesCall;
use ethers::abi::AbiDecode;

pub fn extract_identities_stuff(data: &[u8]) -> RegisterIdentitiesCall {
    RegisterIdentitiesCall::decode(data).unwrap()
}
