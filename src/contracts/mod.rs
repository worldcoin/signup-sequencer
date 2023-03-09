//! Functionality for interacting with smart contracts deployed on chain.
pub mod abi;

use std::sync::Arc;

use anyhow::anyhow;
use clap::Parser;
use ethers::{
    providers::Middleware,
    types::{Address, U256},
};
use semaphore::Field;
use tracing::{error, info, instrument};

use self::abi::BatchingContract as ContractAbi;
use crate::{
    ethereum::{write::TransactionId, Ethereum, ReadProvider},
    prover::{
        batch_insertion::{Identity, Prover as BatchInsertionProver},
        proof::Proof,
    },
};

/// Configuration options for the component responsible for interacting with the
/// contract.
#[derive(Clone, Debug, PartialEq, Eq, Parser)]
#[group(skip)]
pub struct Options {
    /// The address of the identity manager contract.
    #[clap(long, env)]
    pub identity_manager_address: Address,

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
#[derive(Clone, Debug)]
pub struct IdentityManager {
    ethereum:           Ethereum,
    prover:             BatchInsertionProver,
    abi:                ContractAbi<ReadProvider>,
    initial_leaf_value: Field,
    tree_depth:         usize,
}

impl IdentityManager {
    #[instrument(level = "debug", skip_all)]
    pub async fn new(
        options: Options,
        ethereum: Ethereum,
        batch_insertion_prover: BatchInsertionProver,
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
        let abi = ContractAbi::new(
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

        let initial_leaf_value = options.initial_leaf_value;
        let tree_depth = options.tree_depth;

        let identity_manager = Self {
            ethereum,
            prover: batch_insertion_prover,
            abi,
            initial_leaf_value,
            tree_depth,
        };

        Ok(identity_manager)
    }

    #[must_use]
    pub const fn tree_depth(&self) -> usize {
        self.tree_depth
    }

    #[must_use]
    pub const fn batch_size(&self) -> usize {
        self.prover.batch_size()
    }

    #[must_use]
    pub const fn initial_leaf_value(&self) -> Field {
        self.initial_leaf_value
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn prepare_proof(
        &self,
        start_index: usize,
        pre_root: U256,
        post_root: U256,
        identity_commitments: &[Identity],
    ) -> anyhow::Result<Proof> {
        // Ensure that we are not going to submit based on an out of date root anyway.
        self.assert_latest_root(pre_root.into()).await?;

        // We also can't proceed unless the merkle proofs match the known tree depth.
        // Things will break if we do.
        for id in identity_commitments {
            if id.merkle_proof.len() != self.tree_depth {
                return Err(anyhow!(format!(
                    "Length of merkle proof ({len}) did not match tree depth ({depth})",
                    len = id.merkle_proof.len(),
                    depth = self.tree_depth
                )));
            }
        }

        let actual_start_index: u32 = start_index.try_into()?;

        let proof_data: Proof = self
            .prover
            .generate_proof(
                actual_start_index,
                pre_root,
                post_root,
                identity_commitments,
            )
            .await?;

        Ok(proof_data)
    }

    #[instrument(level = "debug", skip_all)]
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

    pub async fn mine_identities(&self, transaction_id: TransactionId) -> anyhow::Result<()> {
        self.ethereum.mine_transaction(transaction_id).await?;
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn assert_latest_root(&self, root: Field) -> anyhow::Result<()> {
        let latest_root = self.abi.latest_root().call().await?;
        let processed_root: U256 = root.into();
        if processed_root == latest_root {
            Ok(())
        } else {
            Err(anyhow::Error::msg(format!("{root} is not latest root.",)))
        }
    }
}

/// A type for an identity manager object that can be sent across threads.
pub type SharedIdentityManager = Arc<IdentityManager>;
