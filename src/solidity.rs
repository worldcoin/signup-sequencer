use std::sync::Arc;

use ethers::{
    core::k256::ecdsa::SigningKey,
    prelude::{
        abigen, Address, Http, LocalWallet, Middleware, Provider, Signer, SignerMiddleware, Wallet,
    },
};

abigen!(
    Semaphore,
    r#"[
        function insertIdentity(uint256 _identityCommitment) public onlyOwner returns (uint256)
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);

const SEMAPHORE_ADDRESS: &str = "0xFE600E2C8023d28219F65C5ED2dDED310737742a";

pub type ContractSigner = SignerMiddleware<Provider<Http>, Wallet<SigningKey>>;
pub type SemaphoreContract = Semaphore<ContractSigner>;

pub async fn initialize_semaphore() -> Result<(Arc<ContractSigner>, SemaphoreContract), eyre::Error>
{
    let provider = Provider::<Http>::try_from("http://localhost:8545")
        .expect("could not instantiate HTTP Provider");
    let chain_id = provider.get_chainid().await.unwrap();

    let wallet = "ee79b5f6e221356af78cf4c36f4f7885a11b67dfcc81c34d80249947330c0f82"
        .parse::<LocalWallet>()?;
    let wallet = wallet.with_chain_id(chain_id.as_u64());

    let signer = SignerMiddleware::new(provider.clone(), wallet);
    let signer = Arc::new(signer);

    let semaphore_address = SEMAPHORE_ADDRESS.parse::<Address>().unwrap();
    let contract = Semaphore::new(semaphore_address, signer.clone());
    Ok((signer, contract))
}
