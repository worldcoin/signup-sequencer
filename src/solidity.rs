use ethers::{
    core::k256::ecdsa::SigningKey,
    prelude::{
        abigen, Address, Http, LocalWallet, Middleware, Provider, Signer, SignerMiddleware, Wallet,
        H160,
    },
};
use eyre::eyre;
use hex_literal::hex;
use std::sync::Arc;

abigen!(
    Semaphore,
    r#"[
        function insertIdentity(uint256 _identityCommitment) public onlyOwner returns (uint256)
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);

const SEMAPHORE_ADDRESS: Address = H160(hex!("FE600E2C8023d28219F65C5ED2dDED310737742a"));
const SIGNING_KEY: [u8; 32] =
    hex!("ee79b5f6e221356af78cf4c36f4f7885a11b67dfcc81c34d80249947330c0f82");

pub type ContractSigner = SignerMiddleware<Provider<Http>, Wallet<SigningKey>>;
pub type SemaphoreContract = Semaphore<ContractSigner>;

pub async fn initialize_semaphore() -> Result<(ContractSigner, SemaphoreContract), eyre::Error> {
    let provider = Provider::<Http>::try_from("http://localhost:8545")
        .expect("could not instantiate HTTP Provider");
    let chain_id: u64 = provider
        .get_chainid()
        .await?
        .try_into()
        .map_err(|e| eyre!("{}", e))?;

    let wallet = LocalWallet::from(SigningKey::from_bytes(&SIGNING_KEY)?).with_chain_id(chain_id);
    let signer = SignerMiddleware::new(provider, wallet);
    let contract = Semaphore::new(SEMAPHORE_ADDRESS, Arc::new(signer.clone()));

    Ok((signer, contract))
}
