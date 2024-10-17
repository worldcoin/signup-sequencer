//! Functionality for interacting with smart contracts deployed on chain.
pub mod abi;
pub mod scanner;

use anyhow::{anyhow, bail, Context};
use ethers::providers::Middleware;
use ethers::types::{H256, U256};
use tracing::{error, info, instrument};

use self::abi::{BridgedWorldId, DeleteIdentitiesCall, WorldId};
use crate::config::Config;
use crate::ethereum::{Ethereum, ReadProvider};
use crate::identity::processor::TransactionId;
use crate::prover::identity::Identity;
use crate::prover::Proof;
use crate::utils::index_packing::unpack_indices;

/// A structure representing the interface to the batch-based identity manager
/// contract.
#[derive(Debug)]
pub struct IdentityManager {
    ethereum: Ethereum,
    abi: WorldId<ReadProvider>,
    secondary_abis: Vec<BridgedWorldId<ReadProvider>>,
    tree_depth: usize,
}

impl IdentityManager {
    // TODO: I don't like these public getters
    pub fn abi(&self) -> &WorldId<ReadProvider> {
        &self.abi
    }

    pub fn secondary_abis(&self) -> &[BridgedWorldId<ReadProvider>] {
        &self.secondary_abis
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn new(config: &Config, ethereum: Ethereum) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let Some(network_config) = &config.network else {
            bail!("Network config is required for IdentityManager.");
        };

        // Check that there is code deployed at the target address.
        let address = network_config.identity_manager_address;
        let code = ethereum.provider().get_code(address, None).await?;
        if code.as_ref().is_empty() {
            error!(
                ?address,
                "No contract code is deployed at the provided address."
            );
        }

        // Connect to the running batching contract.
        let abi = WorldId::new(
            network_config.identity_manager_address,
            ethereum.provider().clone(),
        );

        let operator = abi.identity_operator().call().await?;
        if operator != ethereum.address() {
            error!(?operator, signer = ?ethereum.address(), "Signer is not the identity operator of the identity manager contract.");
            panic!("Cannot currently continue in read-only mode.")
        }

        info!(
            ?address,
            ?operator,
            "Connected to the WorldID Identity Manager"
        );

        let secondary_providers = ethereum.secondary_providers();

        let mut secondary_abis = Vec::new();
        for (chain_id, address) in &network_config.relayed_identity_manager_addresses.0 {
            let provider = secondary_providers
                .get(chain_id)
                .ok_or_else(|| anyhow!("No provider for chain id: {}", chain_id))?;

            let abi = BridgedWorldId::new(*address, provider.clone());
            secondary_abis.push(abi);
        }

        let tree_depth = config.tree.tree_depth;

        let identity_manager = Self {
            ethereum,
            abi,
            secondary_abis,
            tree_depth,
        };

        Ok(identity_manager)
    }

    pub async fn root_history_expiry(&self) -> anyhow::Result<U256> {
        Ok(self.abi.get_root_history_expiry().call().await?)
    }

    #[instrument(level = "debug", skip(self, identity_commitments, proof_data))]
    pub async fn register_identities(
        &self,
        start_index: usize,
        pre_root: U256,
        post_root: U256,
        identity_commitments: Vec<Identity>,
        proof_data: Proof,
    ) -> anyhow::Result<TransactionId> {
        let actual_start_index: u32 = start_index.try_into()?;

        let proof_points_array: [U256; 8] = proof_data.into();
        let identities = identity_commitments
            .iter()
            .map(|id| id.commitment)
            .collect();

        // We want to send the transaction through our ethereum provider rather than
        // directly now. To that end, we create it, and then send it later, waiting for
        // it to complete.
        let register_identities_transaction = self
            .abi
            .register_identities(
                proof_points_array,
                pre_root,
                actual_start_index,
                identities,
                post_root,
            )
            .tx;

        self.ethereum
            .send_transaction(register_identities_transaction, true)
            .await
            .map_err(|tx_err| anyhow!("{}", tx_err.to_string()))
    }

    // TODO: docs
    #[instrument(level = "debug")]
    pub async fn delete_identities(
        &self,
        deletion_proof: Proof,
        packed_deletion_indices: Vec<u8>,
        pre_root: U256,
        post_root: U256,
    ) -> anyhow::Result<TransactionId> {
        let proof_points_array: [U256; 8] = deletion_proof.into();

        let delete_identities_transaction = self
            .abi
            .delete_identities(
                proof_points_array,
                packed_deletion_indices.into(),
                pre_root,
                post_root,
            )
            .tx;

        self.ethereum
            .send_transaction(delete_identities_transaction, true)
            .await
            .map_err(|tx_err| anyhow!("{}", tx_err.to_string()))
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn latest_root(&self) -> anyhow::Result<U256> {
        let latest_root = self.abi.latest_root().call().await?;

        Ok(latest_root)
    }

    /// Fetches the identity commitments from a
    /// `deleteIdentities` transaction by tx hash
    #[instrument(level = "debug", skip_all)]
    pub async fn fetch_deletion_indices_from_tx(
        &self,
        tx_hash: H256,
    ) -> anyhow::Result<Vec<usize>> {
        let provider = self.ethereum.provider();

        let tx = provider
            .get_transaction(tx_hash)
            .await?
            .context("Missing tx")?;

        use ethers::abi::AbiDecode;
        let delete_identities = DeleteIdentitiesCall::decode(&tx.input)?;

        let packed_deletion_indices: &[u8] = delete_identities.packed_deletion_indices.as_ref();
        let indices = unpack_indices(packed_deletion_indices);

        let padding_index = 2u32.pow(self.tree_depth as u32);

        Ok(indices
            .into_iter()
            .filter(|idx| *idx != padding_index)
            .map(|x| x as usize)
            .collect())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn is_root_mined(&self, root: U256) -> anyhow::Result<bool> {
        let (root_on_mainnet, ..) = self.abi.query_root(root).call().await?;

        if root_on_mainnet.is_zero() {
            return Ok(false);
        }

        Ok(true)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn is_root_mined_multi_chain(&self, root: U256) -> anyhow::Result<bool> {
        let (root_on_mainnet, ..) = self.abi.query_root(root).call().await?;

        if root_on_mainnet.is_zero() {
            return Ok(false);
        }

        for bridged_world_id in &self.secondary_abis {
            let root_timestamp = bridged_world_id.root_history(root).call().await?;

            // root_history only returns superseded roots, so we must also check the latest
            // root
            let latest_root = bridged_world_id.latest_root().call().await?;

            // If root is not superseded and it's not the latest root
            // then it's not mined
            if root_timestamp == 0 && root != latest_root {
                return Ok(false);
            }
        }

        Ok(true)
    }
}
