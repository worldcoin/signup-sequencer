mod abi;

use self::abi::BatchingContract as ContractAbi;
use crate::{
    contracts::{IdentityManager, Options},
    ethereum::{Ethereum, TxError, write::TransactionId, ReadProvider},
};
use async_trait::async_trait;
use ethers::{providers::Middleware, types::U256};
use semaphore::Field;
use tracing::{error, info, instrument};

/// A structure representing the interface to the batch-based identity manager
/// contract.
#[derive(Clone, Debug)]
pub struct Contract {
    ethereum:           Ethereum,
    abi:                ContractAbi<ReadProvider>,
    initial_leaf_value: Field,
    tree_depth:         usize,
}

#[async_trait]
impl IdentityManager for Contract {
    #[instrument(level = "debug", skip_all)]
    async fn new(options: Options, ethereum: Ethereum) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        // Check that there is code deployed at the target address.
        let address = options.semaphore_address;
        let code = ethereum.provider().get_code(address, None).await?;
        if code.as_ref().is_empty() {
            error!(
                ?address,
                "No contract code is deployed at the provided address."
            );
        }

        // Connect to the running batching contract.
        let abi = ContractAbi::new(options.semaphore_address, ethereum.provider().clone());

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
            abi,
            initial_leaf_value,
            tree_depth,
        };

        Ok(identity_manager)
    }

    fn tree_depth(&self) -> usize {
        self.tree_depth
    }

    fn initial_leaf_value(&self) -> Field {
        self.initial_leaf_value
    }

    fn group_id(&self) -> U256 {
        // The batch verifier only ever works with one group, so while we still have to
        // contend with groups in the interface we can just hard-code a constant.
        1.into()
    }

    #[instrument(level = "debug", skip_all)]
    async fn is_owner(&self) -> anyhow::Result<bool> {
        info!(address = ?self.ethereum.address(), "My address");
        let owner = self.abi.owner().call().await?;
        info!(?owner, "Fetched owner address");
        Ok(owner == self.ethereum.address())
    }

    #[instrument(level = "debug", skip_all)]
    async fn register_identities(
        &self,
        _identity_commitments: Vec<Field>,
    ) -> Result<TransactionId, TxError> {
        // TODO [Ara] Assert length of merkle tree proofs.
        todo!()
    }

    async fn assert_latest_root(&self, root: Field) -> anyhow::Result<()> {
        let latest_root = self.abi.latest_root().call().await?;
        let processed_root: U256 = root.into();
        if processed_root == latest_root {
            Ok(())
        } else {
            Err(anyhow::Error::msg("Not latest root."))
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn assert_valid_root(&self, root: Field) -> anyhow::Result<()> {
        if self.abi.check_valid_root(root.into()).call().await? {
            Ok(())
        } else {
            Err(anyhow::Error::msg("Root no longer valid"))
        }
    }
}
