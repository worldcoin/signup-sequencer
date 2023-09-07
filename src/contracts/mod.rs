//! Functionality for interacting with smart contracts deployed on chain.
pub mod abi;
pub mod scanner;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;
use clap::Parser;
use ethers::providers::Middleware;
use ethers::types::{Address, U256};
use semaphore::Field;
use tokio::sync::RwLockReadGuard;
use tracing::{error, info, instrument, warn};

use self::abi::{BridgedWorldId, WorldId};
use crate::ethereum::write::TransactionId;
use crate::ethereum::{Ethereum, ReadProvider};
use crate::prover::batch_insertion::ProverConfiguration;
use crate::prover::map::{InsertionProverMap, ReadOnlyInsertionProver};
use crate::prover::{batch_insertion, Proof, ReadOnlyProver};
use crate::serde_utils::JsonStrWrapper;
use crate::server::error::Error as ServerError;

/// Configuration options for the component responsible for interacting with the
/// contract.
#[derive(Clone, Debug, PartialEq, Eq, Parser)]
#[group(skip)]
pub struct Options {
    /// The address of the identity manager contract.
    #[clap(long, env)]
    pub identity_manager_address: Address,

    /// The addresses of world id contracts on secondary chains
    /// mapped by chain id
    #[clap(long, env, default_value = "{}")]
    pub relayed_identity_manager_addresses: JsonStrWrapper<HashMap<u64, Address>>,

    /// The depth of the tree that the contract is working with. This needs to
    /// agree with the verifier in the deployed contract, and also with
    /// `semaphore-mtb`.
    #[clap(long, env, default_value = "10")]
    pub tree_depth: usize,

    /// Initial value of the Merkle tree leaves. Defaults to the initial value
    /// used in the identity manager contract.
    #[clap(
        long,
        env,
        default_value = "0000000000000000000000000000000000000000000000000000000000000000"
    )]
    pub initial_leaf_value: Field,
}

/// A structure representing the interface to the batch-based identity manager
/// contract.
#[derive(Debug)]
pub struct IdentityManager {
    ethereum:             Ethereum,
    insertion_prover_map: InsertionProverMap,
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
        options: Options,
        ethereum: Ethereum,
        insertion_prover_map: InsertionProverMap,
    ) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        // Check that there is code deployed at the target address.
        let address = options.identity_manager_address;
        let code = ethereum.provider().get_code(address, None).await?;
        if code.as_ref().is_empty() {
            error!(
                ?address,
                "No contract code is deployed at the provided address."
            );
        }

        // Connect to the running batching contract.
        let abi = WorldId::new(
            options.identity_manager_address,
            ethereum.provider().clone(),
        );

        let owner = abi.owner().call().await?;
        if owner != ethereum.address() {
            error!(?owner, signer = ?ethereum.address(), "Signer is not the owner of the identity manager contract.");
            panic!("Cannot currently continue in read-only mode.")
        }
        info!(
            ?address,
            ?owner,
            "Connected to the WorldID Identity Manager"
        );

        let secondary_providers = ethereum.secondary_providers();

        let mut secondary_abis = Vec::new();
        for (chain_id, address) in options.relayed_identity_manager_addresses.0 {
            let provider = secondary_providers
                .get(&chain_id)
                .ok_or_else(|| anyhow!("No provider for chain id: {}", chain_id))?;

            let abi = BridgedWorldId::new(address, provider.clone());
            secondary_abis.push(abi);
        }

        let initial_leaf_value = options.initial_leaf_value;
        let tree_depth = options.tree_depth;

        let identity_manager = Self {
            ethereum,
            insertion_prover_map,
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

    pub async fn max_batch_size(&self) -> usize {
        self.insertion_prover_map.read().await.max_batch_size()
    }

    #[must_use]
    pub const fn initial_leaf_value(&self) -> Field {
        self.initial_leaf_value
    }

    /// Validates that merkle proofs are of the correct length against tree
    /// depth
    pub fn validate_merkle_proofs(
        &self,
        identity_commitments: &[batch_insertion::Identity],
    ) -> anyhow::Result<()> {
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

    pub async fn get_suitable_prover(
        &self,
        num_identities: usize,
    ) -> anyhow::Result<ReadOnlyProver<batch_insertion::Prover>> {
        let prover_map = self.insertion_prover_map.read().await;

        match RwLockReadGuard::try_map(prover_map, |map| map.get(num_identities)) {
            Ok(p) => anyhow::Ok(p),
            Err(_) => Err(anyhow!(
                "No available prover for batch size: {num_identities}"
            )),
        }
    }

    #[instrument(level = "debug", skip(prover, identity_commitments))]
    pub async fn prepare_proof(
        prover: ReadOnlyInsertionProver<'_>,
        start_index: usize,
        pre_root: U256,
        post_root: U256,
        identity_commitments: &[batch_insertion::Identity],
    ) -> anyhow::Result<Proof> {
        let batch_size = identity_commitments.len();

        let actual_start_index: u32 = start_index.try_into()?;

        info!(
            "Sending {} identities to prover of batch size {}",
            batch_size,
            prover.batch_size()
        );

        let proof_data: Proof = prover
            .generate_proof(
                actual_start_index,
                pre_root,
                post_root,
                identity_commitments,
            )
            .await?;

        Ok(proof_data)
    }

    #[instrument(level = "debug", skip(self, identity_commitments, proof_data))]
    pub async fn register_identities(
        &self,
        start_index: usize,
        pre_root: U256,
        post_root: U256,
        identity_commitments: Vec<batch_insertion::Identity>,
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

    #[instrument(level = "debug", skip(self))]
    pub async fn mine_identities(&self, transaction_id: TransactionId) -> anyhow::Result<bool> {
        let result = self.ethereum.mine_transaction(transaction_id).await?;
        Ok(result)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn fetch_pending_identities(&self) -> anyhow::Result<Vec<TransactionId>> {
        let pending_identities = self.ethereum.fetch_pending_transactions().await?;

        Ok(pending_identities)
    }

    /// Waits until all the pending transactions have been mined or failed
    #[instrument(level = "debug", skip_all)]
    pub async fn await_clean_slate(&self) -> anyhow::Result<()> {
        // Await for all pending transactions
        let pending_identities = self.fetch_pending_identities().await?;

        for pending_identity_tx in pending_identities {
            // Ignores the result of each transaction - we only care about a clean slate in
            // terms of pending transactions
            drop(self.mine_identities(pending_identity_tx).await);
        }

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn latest_root(&self) -> anyhow::Result<U256> {
        let latest_root = self.abi.latest_root().call().await?;

        Ok(latest_root)
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
    ) -> Result<(), ServerError> {
        let mut map = self.insertion_prover_map.write().await;

        if map.batch_size_exists(batch_size) {
            return Err(ServerError::BatchSizeAlreadyExists);
        }

        let prover = batch_insertion::Prover::new(&ProverConfiguration {
            url: url.to_string(),
            batch_size,
            timeout_s: timeout_seconds,
        })?;

        map.add(batch_size, prover);

        Ok(())
    }

    /// # Errors
    ///
    /// Will return `Err` if the batch size requested for removal doesn't exist
    /// in the prover map.
    pub async fn remove_batch_size(&self, batch_size: usize) -> Result<(), ServerError> {
        let mut map = self.insertion_prover_map.write().await;

        if map.len() == 1 {
            warn!("Attempting to remove the last batch size.");
            return Err(ServerError::CannotRemoveLastBatchSize);
        }

        match map.remove(batch_size) {
            Some(_) => Ok(()),
            None => Err(ServerError::NoSuchBatchSize),
        }
    }

    pub async fn list_batch_sizes(&self) -> Result<Vec<ProverConfiguration>, ServerError> {
        Ok(self
            .insertion_prover_map
            .read()
            .await
            .as_configuration_vec())
    }

    pub async fn has_provers(&self) -> bool {
        self.insertion_prover_map.read().await.len() > 0
    }
}

/// A type for an identity manager object that can be sent across threads.
pub type SharedIdentityManager = Arc<IdentityManager>;
