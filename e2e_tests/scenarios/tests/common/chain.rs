use std::fs::File;
use std::io::BufReader;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use ethers::contract::{Contract, ContractFactory};
use ethers::core::k256::ecdsa::SigningKey;
use ethers::middleware::{Middleware, NonceManagerMiddleware, SignerMiddleware};
use ethers::prelude::{Bytes, LocalWallet, Signer, U256};
use ethers::providers::{Http, Provider};
use ethers::signers::Wallet;
use ethers::types::H160;
use ethers::utils::{Anvil, AnvilInstance};
use tracing::info;

use super::abi as ContractAbi;
use crate::common::prelude::instrument;

type SpecialisedClient =
    NonceManagerMiddleware<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>;
type SharableClient = Arc<SpecialisedClient>;
type SpecialisedFactory = ContractFactory<SpecialisedClient>;
pub type SpecialisedContract = Contract<SpecialisedClient>;

pub struct Chain {
    pub private_key:      SigningKey,
    pub identity_manager: SpecialisedContract,
}

#[instrument(skip_all)]
pub async fn create_chain(chain_addr: String) -> anyhow::Result<Chain> {
    // This private key is taken from tx-sitter configuration in compose.yaml.
    // Env name: TX_SITTER__SERVICE__PREDEFINED__RELAYER__KEY_ID
    let private_key = SigningKey::from_slice(&[
        0xd1, 0x06, 0x07, 0x66, 0x2a, 0x85, 0x42, 0x4f, 0x02, 0xa3, 0x3f, 0xb1, 0xe6, 0xd0, 0x95,
        0xbd, 0x0a, 0xc7, 0x15, 0x43, 0x96, 0xff, 0x09, 0x76, 0x2e, 0x41, 0xf8, 0x2f, 0xf2, 0x23,
        0x3a, 0xaa,
    ])?;
    // This address is taken from signup-sequencer configuration in config.toml.
    // Section: [network], param name: identity_manager_address
    let identity_manager_contract_address =
        H160::from_str("0x48483748eb0446A16cAE79141D0688e3F624Cb73")?;

    let wallet = LocalWallet::from(private_key.clone()).with_chain_id(31337u64);

    let provider = Provider::<Http>::try_from(format!("http://{}", chain_addr))
        .expect("Failed to initialize chain endpoint")
        .interval(Duration::from_millis(500u64));

    // connect the wallet to the provider
    let client = SignerMiddleware::new(provider, wallet.clone());
    let client = NonceManagerMiddleware::new(client, wallet.address());
    let client = Arc::new(client);

    let identity_manager: SpecialisedContract = Contract::new(
        identity_manager_contract_address,
        ContractAbi::IDENTITYMANAGERCONTRACT_ABI.clone(),
        client.clone(),
    );

    Ok(Chain {
        private_key,
        identity_manager,
    })
}
