use std::sync::Arc;
use crate::{identity::Commitment, mimc_tree::MimcTree};
use eyre::{eyre, Result as EyreResult};
use ethers::{core::k256::ecdsa::SigningKey, prelude::{Address, H160, Http, LocalWallet, Middleware, Provider, Signer, SignerMiddleware, Wallet, abigen, builders::Event}};
use hex_literal::hex;

abigen!(
    Semaphore,
    r#"[
        function insertIdentity(uint256 _identityCommitment) public onlyOwner returns (uint256)
        event LeafInsertion(uint256 indexed leaf, uint256 indexed leafIndex)
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

pub async fn parse_identity_commitments(tree: &mut MimcTree, semaphore_contract: SemaphoreContract, starting_block: Option<usize>) -> EyreResult<usize> {
    // TODO read/write from file
    let starting_block = starting_block.unwrap_or(0);
    let filter: Event<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>, LeafInsertionFilter> = semaphore_contract.leaf_insertion_filter().from_block(starting_block);
    let logs = filter.query().await?;
    let mut last_index = 0;
    for event in logs.iter() {
        let index: usize = event.leaf_index.as_u32().try_into()?;
        let leaf = hex::decode(format!("{:x}", event.leaf))?;
        let leaf: Commitment = (&leaf[..]).try_into()?;
        tree.set(index, leaf);
        last_index = index;
    }
    Ok(last_index)
}
