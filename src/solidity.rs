use crate::{identity::Commitment, mimc_tree::MimcTree};
use ethers::{
    core::k256::ecdsa::SigningKey,
    prelude::{
        abigen, builders::Event, Address, Http, LocalWallet, Middleware, Provider, Signer,
        SignerMiddleware, Wallet, H160,
    },
};
use eyre::{eyre, Result as EyreResult};
use hex_literal::hex;
use serde::{Deserialize, Serialize};
use serde_json::Error as SerdeError;
use std::{fs::File, path::Path, sync::Arc};

abigen!(
    Semaphore,
    r#"[
        function insertIdentity(uint256 _identityCommitment) public onlyOwner returns (uint256)
        event LeafInsertion(uint256 indexed leaf, uint256 indexed leafIndex)
    ]"#,
    event_derives(serde::Deserialize, serde::Serialize)
);

const SEMAPHORE_ADDRESS: Address = H160(hex!("266FB396B626621898C87a92efFBA109dE4685F6"));
const SIGNING_KEY: [u8; 32] =
    hex!("ee79b5f6e221356af78cf4c36f4f7885a11b67dfcc81c34d80249947330c0f82");
pub const COMMITMENTS_FILE: &str = "./commitments.json";

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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonCommitment {
    pub last_block:  usize,
    pub commitments: Vec<Commitment>,
}

pub async fn parse_identity_commitments(
    tree: &mut MimcTree,
    semaphore_contract: SemaphoreContract,
) -> EyreResult<usize> {
    let json_file_path = Path::new(COMMITMENTS_FILE);
    let mut last_index = 0;
    let starting_block = match File::open(json_file_path) {
        Ok(file) => {
            let json_commitments: Result<JsonCommitment, SerdeError> =
                serde_json::from_reader(file);
            match json_commitments {
                Ok(json_commitments) => {
                    for &commitment in &json_commitments.commitments {
                        tree.set(last_index, commitment);
                        last_index += 1;
                    }
                    json_commitments.last_block
                }
                Err(_) => 0,
            }
        }
        Err(_) => 0,
    };

    let filter: Event<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>, LeafInsertionFilter> =
        semaphore_contract
            .leaf_insertion_filter()
            .from_block(starting_block);
    let logs = filter.query().await?;
    for event in &logs {
        let index: usize = event.leaf_index.as_u32().try_into()?;
        tree.set(index, event.leaf.into());
        last_index = index;
    }
    Ok(last_index)
}
