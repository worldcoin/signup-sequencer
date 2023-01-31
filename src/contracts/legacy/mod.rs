mod abi;

use self::abi::{LegacyContract as ContractAbi, MemberAddedFilter};
use crate::{
    contracts::{EventStream, IdentityManager, Options},
    ethereum::{write::TransactionId, Ethereum, EventError, ReadProvider, TxError},
};
use anyhow::anyhow;
use async_trait::async_trait;
use core::future;
use ethers::{providers::Middleware, types::U256};
use futures::TryStreamExt;
use semaphore::Field;
use tracing::{error, info, instrument};

pub type MemberAddedEvent = MemberAddedFilter;

/// A structure representing the interface to the legacy identity manager
/// contract.
pub struct Contract {
    ethereum:     Ethereum,
    abi:          ContractAbi<ReadProvider>,
    group_id:     U256,
    tree_depth:   usize,
    initial_leaf: Field,
}

#[async_trait]
impl IdentityManager for Contract {
    #[instrument(level = "debug", skip_all)]
    async fn new(options: Options, ethereum: Ethereum) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        // Sanity check the address
        // TODO: Check that the contract is actually a Semaphore by matching bytecode.
        let address = options.identity_manager_address;
        let code = ethereum.provider().get_code(address, None).await?;
        if code.as_ref().is_empty() {
            error!(
                ?address,
                "No contract code deployed at provided Semaphore address"
            );
            return Err(anyhow!("Invalid Semaphore address"));
        }

        // Connect to Contract
        let semaphore = ContractAbi::new(
            options.identity_manager_address,
            ethereum.provider().clone(),
        );

        // Test contract by calling a view function and make sure we are manager.
        let manager = semaphore.manager().call().await?;
        if manager != ethereum.address() {
            error!(?manager, signer = ?ethereum.address(), "Signer is not the manager of the Semaphore contract");
            // return Err(anyhow!("Signer is not manager"));
            // TODO: If not manager, proceed in read-only mode.
        }
        info!(?address, ?manager, "Connected to Semaphore contract");

        // Make sure the group exists.
        let existing_tree_depth = semaphore.get_depth(options.group_id).call().await?;
        let actual_tree_depth = if existing_tree_depth == 0 {
            if let Some(new_depth) = options.create_group_depth {
                let tx = semaphore
                    .create_group(
                        options.group_id,
                        new_depth.try_into()?,
                        options.initial_leaf_value.to_be_bytes().into(),
                    )
                    .tx;
                ethereum.send_transaction(tx, false).await?;
                new_depth
            } else {
                error!(group_id = ?options.group_id, "Group does not exist");
                return Err(anyhow!("Group does not exist"));
            }
        } else {
            info!(group_id = ?options.group_id, ?existing_tree_depth, "Semaphore group found.");
            usize::from(existing_tree_depth)
        };

        // TODO: Some way to check the initial leaf

        let identity_manager = Self {
            ethereum,
            abi: semaphore,
            group_id: options.group_id,
            tree_depth: actual_tree_depth,
            initial_leaf: options.initial_leaf_value,
        };

        Ok(identity_manager)
    }

    fn tree_depth(&self) -> usize {
        self.tree_depth
    }

    fn initial_leaf_value(&self) -> Field {
        self.initial_leaf
    }

    fn group_id(&self) -> U256 {
        self.group_id
    }

    async fn confirmed_block_number(&self) -> Result<u64, EventError> {
        self.ethereum
            .provider()
            .confirmed_block_number()
            .await
            .map(|num| num.as_u64())
    }

    #[instrument(level = "debug", skip_all)]
    async fn is_owner(&self) -> anyhow::Result<bool> {
        info!(address = ?self.ethereum.address(), "My address");
        let manager = self.abi.manager().call().await?;
        info!(?manager, "Fetched manager address");
        Ok(manager == self.ethereum.address())
    }

    #[instrument(level = "debug", skip_all)]
    async fn register_identities(
        &self,
        identity_commitments: Vec<Field>,
    ) -> Result<TransactionId, TxError> {
        // TODO Make this loop over identities if it gets multiple.
        assert_eq!(
            identity_commitments.len(),
            1,
            "The legacy identity manager can only accept single commitments."
        );
        let identity = identity_commitments.first().unwrap();

        // Send the registration transaction
        let commitment = U256::from(identity.to_be_bytes());
        let receipt = self
            .ethereum
            .send_transaction(self.abi.add_member(self.group_id, commitment).tx, true)
            .await?;
        Ok(receipt)
    }

    async fn assert_latest_root(&self, _: Field) -> anyhow::Result<()> {
        Err(anyhow::Error::msg(
            "Unsupported operation: assert_latest_root",
        ))
    }

    // This is a total hack due to the contract not supporting a `get_root`
    // function.
    #[instrument(level = "debug", skip_all)]
    async fn assert_valid_root(&self, root: Field) -> anyhow::Result<()> {
        // HACK: Abuse the `verifyProof` function.

        let result = self
            .abi
            .verify_proof(
                root.to_be_bytes().into(),
                self.group_id,
                U256::zero(),
                U256::zero(),
                U256::zero(),
                [U256::zero(); 8],
            )
            .call()
            .await
            .expect_err("Proof is invalid");
        // Result will be either `0x09bde339`: `InvalidProof()` (good) or
        // `0x504570e3`: `InvalidRoot()` (bad).
        // See <https://github.com/worldcoin/world-id-example-airdrop/blob/03de53d2cb016ddef28b26e8237e85b62ec385c7/src/Semaphore.sol#L141>
        // See <https://sig.eth.samczsun.com/>
        // HACK: There's really no good way to parse these errors
        let error = result.to_string();
        if error.contains("0x09bde339") {
            return Ok(());
        }
        if error.contains("0x504570e3") {
            return Err(anyhow!("Invalid root"));
        }
        Err(anyhow!("Error verifiying root: {}", result))
    }

    fn fetch_events(&self, starting_block: u64, end_block: Option<u64>) -> Option<EventStream<'_>> {
        // Start the MemberAdded event stream.
        let mut filter = self.abi.member_added_filter().from_block(starting_block);
        if let Some(end_block) = end_block {
            filter = filter.to_block(end_block);
        }
        let stream = self
            .ethereum
            .provider()
            .fetch_events::<MemberAddedEvent>(&filter.filter)
            .try_filter(|event| future::ready(event.event.group_id == self.group_id));
        Some(Box::pin(stream))
    }
}
