use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use chrono::Utc;
use ethers::abi::RawLog;
use ethers::addressbook::Address;
use ethers::contract::EthEvent;
use ethers::middleware::Middleware;
use ethers::prelude::{Log, Topic, ValueOrArray, U256};
use tracing::{error, info, instrument};

use crate::config::Config;
use crate::contracts::abi::{BridgedWorldId, RootAddedFilter, TreeChangeKind, TreeChangedFilter};
use crate::contracts::scanner::BlockScanner;
use crate::contracts::IdentityManager;
use crate::database::query::DatabaseQuery;
use crate::database::types::{BatchEntry, BatchType};
use crate::database::{Database, Error};
use crate::ethereum::{Ethereum, ReadProvider};
use crate::identity_tree::{Canonical, Hash, Intermediate, TreeVersion, TreeWithNextVersion};
use crate::prover::identity::Identity;
use crate::prover::repository::ProverRepository;
use crate::prover::Prover;
use crate::utils::index_packing::pack_indices;
use crate::utils::retry_tx;

pub type TransactionId = String;

#[async_trait]
pub trait IdentityProcessor: Send + Sync + 'static {
    async fn commit_identities(&self, batch: &BatchEntry) -> anyhow::Result<TransactionId>;

    async fn finalize_identities(
        &self,
        processed_tree: &TreeVersion<Intermediate>,
        mined_tree: &TreeVersion<Canonical>,
    ) -> anyhow::Result<()>;

    async fn await_clean_slate(&self) -> anyhow::Result<()>;

    async fn mine_transaction(&self, transaction_id: TransactionId) -> anyhow::Result<bool>;

    async fn tree_init_correction(&self, initial_root_hash: &Hash) -> anyhow::Result<()>;
}

pub struct OnChainIdentityProcessor {
    ethereum:          Ethereum,
    config:            Config,
    database:          Arc<Database>,
    identity_manager:  Arc<IdentityManager>,
    prover_repository: Arc<ProverRepository>,

    mainnet_scanner:    tokio::sync::Mutex<BlockScanner<Arc<ReadProvider>>>,
    mainnet_address:    Address,
    secondary_scanners: tokio::sync::Mutex<HashMap<Address, BlockScanner<Arc<ReadProvider>>>>,
}

#[async_trait]
impl IdentityProcessor for OnChainIdentityProcessor {
    async fn commit_identities(&self, batch: &BatchEntry) -> anyhow::Result<TransactionId> {
        if batch.batch_type == BatchType::Insertion {
            let prover = self
                .prover_repository
                .get_suitable_insertion_prover(batch.data.0.identities.len())
                .await?;

            info!(
                num_updates = batch.data.0.identities.len(),
                batch_size = prover.batch_size(),
                "Insertion batch",
            );

            self.insert_identities(&prover, batch).await
        } else {
            let prover = self
                .prover_repository
                .get_suitable_deletion_prover(batch.data.0.identities.len())
                .await?;

            info!(
                num_updates = batch.data.0.identities.len(),
                batch_size = prover.batch_size(),
                "Deletion batch"
            );

            self.delete_identities(&prover, batch).await
        }
    }

    async fn finalize_identities(
        &self,
        processed_tree: &TreeVersion<Intermediate>,
        mined_tree: &TreeVersion<Canonical>,
    ) -> anyhow::Result<()> {
        let mainnet_logs = self.fetch_mainnet_logs().await?;

        self.finalize_mainnet_roots(
            processed_tree,
            &mainnet_logs,
            self.config.app.max_epoch_duration,
        )
        .await?;

        let mut roots = Self::extract_roots_from_mainnet_logs(mainnet_logs);
        roots.extend(self.fetch_secondary_logs().await?);

        self.finalize_secondary_roots(mined_tree, roots).await?;

        Ok(())
    }

    async fn await_clean_slate(&self) -> anyhow::Result<()> {
        // Await for all pending transactions
        let pending_identities = self.fetch_pending_identities().await?;

        for pending_identity_tx in pending_identities {
            // Ignores the result of each transaction - we only care about a clean slate in
            // terms of pending transactions
            drop(self.mine_transaction(pending_identity_tx).await);
        }

        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    async fn mine_transaction(&self, transaction_id: TransactionId) -> anyhow::Result<bool> {
        let result = self.ethereum.mine_transaction(transaction_id).await?;

        Ok(result)
    }

    async fn tree_init_correction(&self, initial_root_hash: &Hash) -> anyhow::Result<()> {
        // Prefetch latest root & mark it as mined
        let root_hash = self.identity_manager.latest_root().await?;
        let root_hash = root_hash.into();

        // We don't store the initial root in the database, so we have to skip this step
        // if the contract root hash is equal to initial root hash
        if root_hash != *initial_root_hash {
            // Note that we don't have a way of queuing a root here for
            // finalization. so it's going to stay as "processed"
            // until the next root is mined. self.database.
            self.database
                .mark_root_as_processed_and_delete_batches_tx(&root_hash)
                .await?;
        } else {
            // Db is either empty or we're restarting with a new contract/chain
            // so we should mark everything as pending
            retry_tx!(self.database.pool, tx, {
                tx.mark_all_as_pending().await?;
                tx.delete_all_batches().await?;
                Result::<(), Error>::Ok(())
            })
            .await?;
        }

        Ok(())
    }
}

impl OnChainIdentityProcessor {
    pub async fn new(
        ethereum: Ethereum,
        config: Config,
        database: Arc<Database>,
        identity_manager: Arc<IdentityManager>,
        prover_repository: Arc<ProverRepository>,
    ) -> anyhow::Result<Self> {
        let mainnet_abi = identity_manager.abi();
        let secondary_abis = identity_manager.secondary_abis();

        let mainnet_scanner = tokio::sync::Mutex::new(
            BlockScanner::new_latest(
                mainnet_abi.client().clone(),
                config.app.scanning_window_size,
            )
            .await?
            .with_offset(config.app.scanning_chain_head_offset),
        );

        let secondary_scanners = tokio::sync::Mutex::new(
            Self::init_secondary_scanners(secondary_abis, config.app.scanning_window_size).await?,
        );

        let mainnet_address = mainnet_abi.address();
        Ok(Self {
            ethereum,
            config,
            database,
            identity_manager,
            prover_repository,
            mainnet_scanner,
            mainnet_address,
            secondary_scanners,
        })
    }

    async fn init_secondary_scanners<T>(
        providers: &[BridgedWorldId<T>],
        scanning_window_size: u64,
    ) -> anyhow::Result<HashMap<Address, BlockScanner<Arc<T>>>>
    where
        T: Middleware,
        <T as Middleware>::Error: 'static,
    {
        let mut secondary_scanners = HashMap::new();

        for bridged_abi in providers {
            let scanner =
                BlockScanner::new_latest(bridged_abi.client().clone(), scanning_window_size)
                    .await?;

            let address = bridged_abi.address();

            secondary_scanners.insert(address, scanner);
        }

        Ok(secondary_scanners)
    }

    #[instrument(level = "info", skip_all)]
    async fn insert_identities(
        &self,
        prover: &Prover,
        batch: &BatchEntry,
    ) -> anyhow::Result<TransactionId> {
        self.validate_merkle_proofs(&batch.data.0.identities)?;
        let start_index = *batch.data.0.indexes.first().expect("Should exist.");
        let pre_root: U256 = batch.prev_root.expect("Should exist.").into();
        let post_root: U256 = batch.next_root.into();

        // We prepare the proof before reserving a slot in the pending identities
        let proof = crate::prover::proof::prepare_insertion_proof(
            prover,
            start_index,
            pre_root,
            &batch.data.0.identities,
            post_root,
        )
        .await?;

        info!(
            start_index,
            ?pre_root,
            ?post_root,
            "Submitting insertion batch"
        );

        // With all the data prepared we can submit the identities to the on-chain
        // identity manager and wait for that transaction to be mined.
        let transaction_id = self
            .identity_manager
            .register_identities(
                start_index,
                pre_root,
                post_root,
                batch.data.0.identities.clone(),
                proof,
            )
            .await
            .map_err(|e| {
                error!(?e, "Failed to insert identity to contract.");
                e
            })?;

        info!(
            start_index,
            ?pre_root,
            ?post_root,
            ?transaction_id,
            "Insertion batch submitted"
        );

        Ok(transaction_id)
    }

    #[instrument(level = "info", skip_all)]
    async fn delete_identities(
        &self,
        prover: &Prover,
        batch: &BatchEntry,
    ) -> anyhow::Result<TransactionId> {
        self.validate_merkle_proofs(&batch.data.0.identities)?;
        let pre_root: U256 = batch.prev_root.expect("Should exist.").into();
        let post_root: U256 = batch.next_root.into();
        let deletion_indices: Vec<_> = batch.data.0.indexes.iter().map(|&v| v as u32).collect();

        // We prepare the proof before reserving a slot in the pending identities
        let proof = crate::prover::proof::prepare_deletion_proof(
            prover,
            pre_root,
            deletion_indices.clone(),
            batch.data.0.identities.clone(),
            post_root,
        )
        .await?;

        let packed_deletion_indices = pack_indices(&deletion_indices);

        info!(?pre_root, ?post_root, "Submitting deletion batch");

        // With all the data prepared we can submit the identities to the on-chain
        // identity manager and wait for that transaction to be mined.
        let transaction_id = self
            .identity_manager
            .delete_identities(proof, packed_deletion_indices, pre_root, post_root)
            .await
            .map_err(|e| {
                error!(?e, "Failed to insert identity to contract.");
                e
            })?;

        info!(
            ?pre_root,
            ?post_root,
            ?transaction_id,
            "Deletion batch submitted"
        );

        Ok(transaction_id)
    }

    #[instrument(level = "debug", skip_all)]
    async fn fetch_pending_identities(&self) -> anyhow::Result<Vec<TransactionId>> {
        let pending_identities = self.ethereum.fetch_pending_transactions().await?;

        Ok(pending_identities)
    }

    /// Validates that merkle proofs are of the correct length against tree
    /// depth
    pub fn validate_merkle_proofs(&self, identity_commitments: &[Identity]) -> anyhow::Result<()> {
        let tree_depth = self.config.tree.tree_depth;
        for id in identity_commitments {
            if id.merkle_proof.len() != tree_depth {
                return Err(anyhow!(format!(
                    "Length of merkle proof ({len}) did not match tree depth ({depth})",
                    len = id.merkle_proof.len(),
                    depth = tree_depth
                )));
            }
        }

        Ok(())
    }

    async fn fetch_mainnet_logs(&self) -> anyhow::Result<Vec<Log>>
    where
        <ReadProvider as Middleware>::Error: 'static,
    {
        let mainnet_topics = [
            Some(Topic::from(TreeChangedFilter::signature())),
            None,
            None,
            None,
        ];

        let mainnet_address = Some(ValueOrArray::Value(self.mainnet_address));

        let mut mainnet_scanner = self.mainnet_scanner.lock().await;

        let mainnet_logs = mainnet_scanner
            .next(mainnet_address, mainnet_topics.clone())
            .await?;

        Ok(mainnet_logs)
    }

    async fn fetch_secondary_logs(&self) -> anyhow::Result<Vec<U256>>
    where
        <ReadProvider as Middleware>::Error: 'static,
    {
        let bridged_topics = [
            Some(Topic::from(RootAddedFilter::signature())),
            None,
            None,
            None,
        ];

        let mut secondary_logs = vec![];

        {
            let mut secondary_scanners = self.secondary_scanners.lock().await;

            for (address, scanner) in secondary_scanners.iter_mut() {
                let logs = scanner
                    .next(Some(ValueOrArray::Value(*address)), bridged_topics.clone())
                    .await?;

                secondary_logs.extend(logs);
            }
        }

        let roots = Self::extract_roots_from_secondary_logs(&secondary_logs);

        Ok(roots)
    }

    #[instrument(level = "info", skip_all)]
    async fn finalize_mainnet_roots(
        &self,
        processed_tree: &TreeVersion<Intermediate>,
        logs: &[Log],
        max_epoch_duration: Duration,
    ) -> Result<(), anyhow::Error> {
        for log in logs {
            let Some(event) = Self::raw_log_to_tree_changed(log) else {
                continue;
            };

            let pre_root = event.pre_root;
            let post_root = event.post_root;
            let kind = TreeChangeKind::from(event.kind);

            info!(?pre_root, ?post_root, ?kind, "Mining batch");

            // Double check
            if !self.identity_manager.is_root_mined(post_root).await? {
                continue;
            }

            self.database
                .mark_root_as_processed_tx(&post_root.into())
                .await?;

            info!(?pre_root, ?post_root, ?kind, "Batch mined");

            if kind == TreeChangeKind::Deletion {
                // NOTE: We must do this before updating the tree
                //       because we fetch commitments from the processed tree
                //       before they are deleted
                self.update_eligible_recoveries(processed_tree, log, max_epoch_duration)
                    .await?;
            }

            let updates_count = processed_tree.apply_updates_up_to(post_root.into());

            info!(updates_count, ?pre_root, ?post_root, "Mined tree updated");
        }

        Ok(())
    }

    #[instrument(level = "info", skip_all)]
    async fn finalize_secondary_roots(
        &self,
        finalized_tree: &TreeVersion<Canonical>,
        roots: Vec<U256>,
    ) -> Result<(), anyhow::Error> {
        for root in roots {
            info!(?root, "Finalizing root");

            // Check if mined on all L2s
            if !self
                .identity_manager
                .is_root_mined_multi_chain(root)
                .await?
            {
                continue;
            }

            self.database.mark_root_as_mined_tx(&root.into()).await?;
            finalized_tree.apply_updates_up_to(root.into());

            info!(?root, "Root finalized");
        }

        Ok(())
    }

    fn extract_roots_from_mainnet_logs(mainnet_logs: Vec<Log>) -> Vec<U256> {
        let mut roots = vec![];
        for log in mainnet_logs {
            let Some(event) = Self::raw_log_to_tree_changed(&log) else {
                continue;
            };

            let post_root = event.post_root;

            roots.push(post_root);
        }
        roots
    }

    fn raw_log_to_tree_changed(log: &Log) -> Option<TreeChangedFilter> {
        let raw_log = RawLog::from((log.topics.clone(), log.data.to_vec()));

        TreeChangedFilter::decode_log(&raw_log).ok()
    }

    fn extract_roots_from_secondary_logs(logs: &[Log]) -> Vec<U256> {
        let mut roots = vec![];

        for log in logs {
            let raw_log = RawLog::from((log.topics.clone(), log.data.to_vec()));
            if let Ok(event) = RootAddedFilter::decode_log(&raw_log) {
                roots.push(event.root);
            }
        }

        roots
    }

    async fn update_eligible_recoveries(
        &self,
        processed_tree: &TreeVersion<Intermediate>,
        log: &Log,
        max_epoch_duration: Duration,
    ) -> anyhow::Result<()> {
        retry_tx!(self.database.pool, tx, {
            let tx_hash = log.transaction_hash.context("Missing tx hash")?;
            let commitments = self
                .identity_manager
                .fetch_deletion_indices_from_tx(tx_hash)
                .await
                .context("Could not fetch deletion indices from tx")?;

            let commitments = processed_tree.commitments_by_indices(commitments.iter().copied());
            let commitments: Vec<U256> = commitments
                .into_iter()
                .map(std::convert::Into::into)
                .collect();

            // Fetch the root history expiry time on chain
            let root_history_expiry = self.identity_manager.root_history_expiry().await?;

            // Use the root history expiry to calculate the eligibility timestamp for the
            // new insertion
            let root_history_expiry_duration =
                chrono::Duration::seconds(root_history_expiry.as_u64() as i64);
            let max_epoch_duration = chrono::Duration::from_std(max_epoch_duration)?;

            let delay = root_history_expiry_duration + max_epoch_duration;

            let eligibility_timestamp = Utc::now() + delay;

            // Check if any deleted commitments correspond with entries in the
            // recoveries table and insert the new commitment into the unprocessed
            // identities table with the proper eligibility timestamp
            let deleted_recoveries = tx.delete_recoveries(commitments).await?;

            // For each deletion, if there is a corresponding recovery, insert a new
            // identity with the specified eligibility timestamp
            for recovery in deleted_recoveries {
                tx.insert_new_identity(recovery.new_commitment, eligibility_timestamp)
                    .await?;
            }

            Ok(())
        })
        .await
    }
}

pub struct OffChainIdentityProcessor {
    committed_batches: Arc<Mutex<Vec<BatchEntry>>>,
    database:          Arc<Database>,
}

#[async_trait]
impl IdentityProcessor for OffChainIdentityProcessor {
    async fn commit_identities(&self, batch: &BatchEntry) -> anyhow::Result<TransactionId> {
        self.add_batch(batch.clone());
        Ok(batch.id.to_string())
    }

    async fn finalize_identities(
        &self,
        processed_tree: &TreeVersion<Intermediate>,
        mined_tree: &TreeVersion<Canonical>,
    ) -> anyhow::Result<()> {
        let batches = {
            let mut committed_batches = self.committed_batches.lock().unwrap();
            let copied = committed_batches.clone();
            committed_batches.clear();
            copied
        };

        for batch in batches.iter() {
            if batch.batch_type == BatchType::Deletion {
                self.update_eligible_recoveries(batch).await?;
            }

            // With current flow it is required to mark root as processed first as this is
            // how required mined_at field is set
            self.database
                .mark_root_as_processed_tx(&batch.next_root)
                .await?;
            self.database
                .mark_root_as_mined_tx(&batch.next_root)
                .await?;
            processed_tree.apply_updates_up_to(batch.next_root);
            mined_tree.apply_updates_up_to(batch.next_root);
        }

        Ok(())
    }

    async fn await_clean_slate(&self) -> anyhow::Result<()> {
        // For off chain mode we don't need to wait as transactions are instantly done
        Ok(())
    }

    async fn mine_transaction(&self, _transaction_id: TransactionId) -> anyhow::Result<bool> {
        // For off chain mode we don't mine transactions, so we treat all of them as
        // mined
        Ok(true)
    }

    async fn tree_init_correction(&self, _initial_root_hash: &Hash) -> anyhow::Result<()> {
        // For off chain mode we don't correct tree at all
        Ok(())
    }
}

impl OffChainIdentityProcessor {
    pub async fn new(database: Arc<Database>) -> anyhow::Result<Self> {
        Ok(OffChainIdentityProcessor {
            committed_batches: Arc::new(Mutex::new(Default::default())),
            database,
        })
    }

    fn add_batch(&self, batch_entry: BatchEntry) {
        let mut committed_batches = self.committed_batches.lock().unwrap();

        committed_batches.push(batch_entry);
    }

    async fn update_eligible_recoveries(&self, batch: &BatchEntry) -> anyhow::Result<()> {
        retry_tx!(self.database.pool, tx, {
            let commitments: Vec<U256> =
                batch.data.identities.iter().map(|v| v.commitment).collect();
            let eligibility_timestamp = Utc::now();

            // Check if any deleted commitments correspond with entries in the
            // recoveries table and insert the new commitment into the unprocessed
            // identities table with the proper eligibility timestamp
            let deleted_recoveries = tx.delete_recoveries(commitments).await?;

            // For each deletion, if there is a corresponding recovery, insert a new
            // identity with the specified eligibility timestamp
            for recovery in deleted_recoveries {
                tx.insert_new_identity(recovery.new_commitment, eligibility_timestamp)
                    .await?;
            }

            Ok(())
        })
        .await
    }
}
