use crate::common::abi::IWorldIDIdentityManager;
use crate::common::prelude::instrument;
use alloy::primitives::Address;
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use k256::ecdsa::SigningKey;
use std::str::FromStr;
use std::time::Duration;

pub type SpecialisedContract =
    IWorldIDIdentityManager::IWorldIDIdentityManagerInstance<DynProvider>;
pub struct Chain {
    pub private_key: SigningKey,
    pub identity_manager: SpecialisedContract,
}

#[instrument(skip_all)]
pub async fn create_chain(chain_addr: String) -> anyhow::Result<Chain> {
    // Matches the predefined relayer key in compose.yaml.
    let private_key = SigningKey::from_slice(&hex_literal::hex!(
        "d10607662a85424f02a33fb1e6d095bd0ac7154396ff09762e41f82ff2233aaa"
    ))?;
    // Matches the identity manager address in config.toml.
    let address = Address::from_str("0x48483748eb0446A16cAE79141D0688e3F624Cb73")?;
    let wallet = PrivateKeySigner::from(private_key.clone());
    let client = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(format!("http://{chain_addr}").parse()?)
        .erased();
    client
        .client()
        .set_poll_interval(Duration::from_millis(500));
    let identity_manager = IWorldIDIdentityManager::new(address, client);
    Ok(Chain {
        private_key,
        identity_manager,
    })
}
