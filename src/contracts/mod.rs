//! Functionality for interacting with smart contracts deployed on chain.
pub mod abi;
pub mod scanner;

use anyhow::{anyhow, Context};
use ethers::providers::Middleware;
use ethers::types::{H256, U256};
use semaphore::Field;
use tokio::sync::{RwLock, RwLockReadGuard};
use tracing::{error, info, instrument, warn};

use self::abi::{BridgedWorldId, DeleteIdentitiesCall, WorldId};
use crate::config::Config;
use crate::ethereum::{Ethereum, ReadProvider};
use crate::identity::transaction_manager::TransactionId;
use crate::prover::identity::Identity;
use crate::prover::{Proof, Prover, ProverConfig, ProverMap, ProverType};
use crate::server::error::Error as ServerError;
use crate::utils::index_packing::unpack_indices;

/// A structure representing the interface to the batch-based identity manager
/// contract.
#[derive(Debug)]
pub struct IdentityManager {
    ethereum:             Ethereum,
    insertion_prover_map: RwLock<ProverMap>,
    deletion_prover_map:  RwLock<ProverMap>,
    abi:                  WorldId<ReadProvider>,
    secondary_abis:       Vec<BridgedWorldId<ReadProvider>>,
    initial_leaf_value:   Field,
    tree_depth:           usize,
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
    pub async fn new(
        config: &Config,
        ethereum: Ethereum,
        insertion_prover_map: ProverMap,
        deletion_prover_map: ProverMap,
    ) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        // Check that there is code deployed at the target address.
        let address = config.network.identity_manager_address;
        let code = ethereum.provider().get_code(address, None).await?;
        if code.as_ref().is_empty() {
            error!(
                ?address,
                "No contract code is deployed at the provided address."
            );
        }

        // Connect to the running batching contract.
        let abi = WorldId::new(
            config.network.identity_manager_address,
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
        for (chain_id, address) in &config.network.relayed_identity_manager_addresses.0 {
            let provider = secondary_providers
                .get(chain_id)
                .ok_or_else(|| anyhow!("No provider for chain id: {}", chain_id))?;

            let abi = BridgedWorldId::new(*address, provider.clone());
            secondary_abis.push(abi);
        }

        let initial_leaf_value = config.tree.initial_leaf_value;
        let tree_depth = config.tree.tree_depth;

        let insertion_prover_map = RwLock::new(insertion_prover_map);
        let deletion_prover_map = RwLock::new(deletion_prover_map);

        let identity_manager = Self {
            ethereum,
            insertion_prover_map,
            deletion_prover_map,
            abi,
            secondary_abis,
            initial_leaf_value,
            tree_depth,
        };

        Ok(identity_manager)
    }

    #[must_use]
    pub const fn tree_depth(&self) -> usize {
        self.tree_depth
    }

    pub async fn max_insertion_batch_size(&self) -> usize {
        self.insertion_prover_map.read().await.max_batch_size()
    }

    pub async fn max_deletion_batch_size(&self) -> usize {
        self.deletion_prover_map.read().await.max_batch_size()
    }

    #[must_use]
    pub const fn initial_leaf_value(&self) -> Field {
        self.initial_leaf_value
    }

    /// Validates that merkle proofs are of the correct length against tree
    /// depth
    pub fn validate_merkle_proofs(&self, identity_commitments: &[Identity]) -> anyhow::Result<()> {
        for id in identity_commitments {
            if id.merkle_proof.len() != self.tree_depth {
                return Err(anyhow!(format!(
                    "Length of merkle proof ({len}) did not match tree depth ({depth})",
                    len = id.merkle_proof.len(),
                    depth = self.tree_depth
                )));
            }
        }

        Ok(())
    }

    pub async fn get_suitable_insertion_prover(
        &self,
        num_identities: usize,
    ) -> anyhow::Result<RwLockReadGuard<Prover>> {
        let prover_map = self.insertion_prover_map.read().await;

        match RwLockReadGuard::try_map(prover_map, |map| map.get(num_identities)) {
            Ok(p) => anyhow::Ok(p),
            Err(_) => Err(anyhow!(
                "No available prover for batch size: {num_identities}"
            )),
        }
    }

    pub async fn get_suitable_deletion_prover(
        &self,
        num_identities: usize,
    ) -> anyhow::Result<RwLockReadGuard<Prover>> {
        let prover_map = self.deletion_prover_map.read().await;

        match RwLockReadGuard::try_map(prover_map, |map| map.get(num_identities)) {
            Ok(p) => anyhow::Ok(p),
            Err(_) => Err(anyhow!(
                "No available prover for batch size: {num_identities}"
            )),
        }
    }

    pub async fn root_history_expiry(&self) -> anyhow::Result<U256> {
        Ok(self.abi.get_root_history_expiry().call().await?)
    }

    #[instrument(level = "debug", skip(prover, identity_commitments))]
    pub async fn prepare_insertion_proof(
        prover: &Prover,
        start_index: usize,
        pre_root: U256,
        identity_commitments: &[Identity],
        post_root: U256,
    ) -> anyhow::Result<Proof> {
        let batch_size = identity_commitments.len();

        let actual_start_index: u32 = start_index.try_into()?;

        info!(
            "Sending {} identities to prover of batch size {}",
            batch_size,
            prover.batch_size()
        );

        let proof_data: Proof = prover
            .generate_insertion_proof(
                actual_start_index,
                pre_root,
                post_root,
                identity_commitments,
            )
            .await?;

        Ok(proof_data)
    }

    #[instrument(level = "debug", skip(prover, identity_commitments))]
    pub async fn prepare_deletion_proof(
        prover: &Prover,
        pre_root: U256,
        deletion_indices: Vec<u32>,
        identity_commitments: Vec<Identity>,
        post_root: U256,
    ) -> anyhow::Result<Proof> {
        info!(
            "Sending {} identities to prover of batch size {}",
            identity_commitments.len(),
            prover.batch_size()
        );

        let proof_data: Proof = prover
            .generate_deletion_proof(pre_root, post_root, deletion_indices, identity_commitments)
            .await?;

        Ok(proof_data)
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

    #[instrument(level = "debug", skip(self))]
    pub async fn mine_transaction(&self, transaction_id: TransactionId) -> anyhow::Result<bool> {
        let result = self.ethereum.mine_transaction(transaction_id).await?;

        Ok(result)
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

    /// # Errors
    ///
    /// Will return `Err` if the provided batch size already exists.
    pub async fn add_batch_size(
        &self,
        url: &impl ToString,
        batch_size: usize,
        timeout_seconds: u64,
        prover_type: ProverType,
    ) -> Result<(), ServerError> {
        let mut map = match prover_type {
            ProverType::Insertion => self.insertion_prover_map.write().await,
            ProverType::Deletion => self.deletion_prover_map.write().await,
        };

        if map.batch_size_exists(batch_size) {
            return Err(ServerError::BatchSizeAlreadyExists);
        }

        let prover = Prover::new(&ProverConfig {
            url: url.to_string(),
            batch_size,
            prover_type,
            timeout_s: timeout_seconds,
        })?;

        map.add(batch_size, prover);

        Ok(())
    }

    /// # Errors
    ///
    /// Will return `Err` if the batch size requested for removal doesn't exist
    /// in the prover map.
    pub async fn remove_batch_size(
        &self,
        batch_size: usize,
        prover_type: ProverType,
    ) -> Result<(), ServerError> {
        let mut map = match prover_type {
            ProverType::Insertion => self.insertion_prover_map.write().await,
            ProverType::Deletion => self.deletion_prover_map.write().await,
        };

        if map.len() == 1 {
            warn!("Attempting to remove the last batch size.");
            return Err(ServerError::CannotRemoveLastBatchSize);
        }

        match map.remove(batch_size) {
            Some(_) => Ok(()),
            None => Err(ServerError::NoSuchBatchSize),
        }
    }

    pub async fn list_batch_sizes(&self) -> Result<Vec<ProverConfig>, ServerError> {
        let mut provers = self
            .insertion_prover_map
            .read()
            .await
            .as_configuration_vec();

        provers.extend(self.deletion_prover_map.read().await.as_configuration_vec());

        Ok(provers)
    }

    pub async fn has_insertion_provers(&self) -> bool {
        self.insertion_prover_map.read().await.len() > 0
    }

    pub async fn has_deletion_provers(&self) -> bool {
        self.deletion_prover_map.read().await.len() > 0
    }
}
