use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use ethers::abi::RawLog;
use ethers::addressbook::Address;
use ethers::contract::EthEvent;
use ethers::middleware::Middleware;
use ethers::prelude::{Log, Topic, ValueOrArray};
use tokio::sync::Notify;
use tracing::{error, info, instrument};

use crate::config::Config;
use crate::contracts::abi::{BridgedWorldId, RootAddedFilter, TreeChangeKind, TreeChangedFilter};
use crate::contracts::scanner::BlockScanner;
use crate::contracts::IdentityManager;
use crate::database::methods::DbMethods;
use crate::database::types::{BatchEntry, BatchType};
use crate::database::{Database, IsolationLevel};
use crate::ethereum::{Ethereum, ReadProvider};
use crate::identity_tree::{Hash, ProcessedStatus};
use crate::prover::identity::Identity;
use crate::prover::repository::ProverRepository;
use crate::prover::Prover;
use crate::retry_tx;
use crate::utils::index_packing::pack_indices;

pub type TransactionId = String;

#[async_trait]
pub trait IdentityProcessor: Send + Sync + 'static {
    async fn commit_identities(&self, batch: &BatchEntry) -> anyhow::Result<TransactionId>;

    async fn finalize_identities(&self, sync_tree_notify: &Arc<Notify>) -> anyhow::Result<()>;

    async fn mine_transaction(&self, transaction_id: TransactionId) -> anyhow::Result<bool>;

    async fn tree_init_correction(&self, initial_root_hash: &Hash) -> anyhow::Result<()>;

    async fn latest_root(&self) -> anyhow::Result<Option<Hash>>;
}

pub struct OnChainIdentityProcessor {
    ethereum: Ethereum,
    config: Config,
    database: Arc<Database>,
    identity_manager: Arc<IdentityManager>,
    prover_repository: Arc<ProverRepository>,

    mainnet_scanner: tokio::sync::Mutex<BlockScanner<Arc<ReadProvider>>>,
    mainnet_address: Address,
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

    async fn finalize_identities(&self, sync_tree_notify: &Arc<Notify>) -> anyhow::Result<()> {
        let mainnet_logs = self.fetch_mainnet_logs().await?;

        self.finalize_mainnet_roots(sync_tree_notify, &mainnet_logs)
            .await?;

        let mut roots = Self::extract_roots_from_mainnet_logs(mainnet_logs);
        roots.extend(self.fetch_secondary_logs().await?);

        self.finalize_secondary_roots(sync_tree_notify, roots)
            .await?;

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

        // it's enough to run with read committed here
        // since in the worst case another instance of the sequencer
        // will try to do the same thing but with a later root
        // in such a case the state will be corrected later in the program
        let mut tx = self
            .database
            .begin_tx(IsolationLevel::ReadCommitted)
            .await?;

        // We don't store the initial root in the database, so we have to skip this step
        // if the contract root hash is equal to initial root hash
        if root_hash != *initial_root_hash {
            // Note that we don't have a way of queuing a root here for
            // finalization. so it's going to stay as "processed"
            // until the next root is mined. self.database.
            tx.mark_root_as_processed(&root_hash).await?;
        } else {
            // Db is either empty or we're restarting with a new contract/chain
            // so we should mark everything as pending
            tx.mark_all_as_pending().await?;
        }

        tx.commit().await?;

        Ok(())
    }

    async fn latest_root(&self) -> anyhow::Result<Option<Hash>> {
        Ok(Some(self.identity_manager.latest_root().await?.into()))
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
        let pre_root: Hash = batch.prev_root.expect("Should exist.");
        let post_root: Hash = batch.next_root;

        // We prepare the proof before reserving a slot in the pending identities
        let proof = crate::prover::proof::prepare_insertion_proof(
            prover,
            start_index,
            pre_root.into(),
            &batch.data.0.identities,
            post_root.into(),
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
                pre_root.into(),
                post_root.into(),
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
        let pre_root: Hash = batch.prev_root.expect("Should exist.");
        let post_root: Hash = batch.next_root;
        let deletion_indices: Vec<_> = batch.data.0.indexes.iter().map(|&v| v as u32).collect();

        // We prepare the proof before reserving a slot in the pending identities
        let proof = crate::prover::proof::prepare_deletion_proof(
            prover,
            pre_root.into(),
            deletion_indices.clone(),
            batch.data.0.identities.clone(),
            post_root.into(),
        )
        .await?;

        let packed_deletion_indices = pack_indices(&deletion_indices);

        info!(?pre_root, ?post_root, "Submitting deletion batch");

        // With all the data prepared we can submit the identities to the on-chain
        // identity manager and wait for that transaction to be mined.
        let transaction_id = self
            .identity_manager
            .delete_identities(
                proof,
                packed_deletion_indices,
                pre_root.into(),
                post_root.into(),
            )
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

    async fn fetch_secondary_logs(&self) -> anyhow::Result<Vec<Hash>>
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
        sync_tree_notify: &Arc<Notify>,
        logs: &[Log],
    ) -> Result<(), anyhow::Error> {
        for log in logs {
            let Some(event) = Self::raw_log_to_tree_changed(log) else {
                continue;
            };

            let pre_root: Hash = event.pre_root.into();
            let post_root: Hash = event.post_root.into();
            let kind = TreeChangeKind::from(event.kind);

            info!(?pre_root, ?post_root, ?kind, "Mining batch");

            // Double check
            if !self
                .identity_manager
                .is_root_mined(post_root.into())
                .await?
            {
                continue;
            }

            retry_tx!(self.database.pool, tx, {
                // With current flow it is required to mark root as processed first as this is
                // how required mined_at field is set, We set proper state only if not set
                // previously.
                let root_state = tx.get_root_state(&post_root).await?;
                if let Some(root_state) = root_state {
                    if root_state.status == ProcessedStatus::Processed
                        || root_state.status == ProcessedStatus::Mined
                    {
                        return Ok::<(), anyhow::Error>(());
                    }
                }

                tx.mark_root_as_processed(&post_root).await?;

                Ok::<(), anyhow::Error>(())
            })
            .await?;

            info!(?pre_root, ?post_root, ?kind, "Batch mined");

            sync_tree_notify.notify_one();
        }

        Ok(())
    }

    #[instrument(level = "info", skip_all)]
    async fn finalize_secondary_roots(
        &self,
        sync_tree_notify: &Arc<Notify>,
        roots: Vec<Hash>,
    ) -> Result<(), anyhow::Error> {
        for root in roots {
            info!(?root, "Finalizing root");

            // Check if mined on all L2s
            if !self
                .identity_manager
                .is_root_mined_multi_chain(root.into())
                .await?
            {
                continue;
            }

            retry_tx!(self.database.pool, tx, {
                // With current flow it is required to mark root as processed first as this is
                // how required mined_at field is set, We set proper state only if not set
                // previously.
                let root_state = tx.get_root_state(&root).await?;
                match root_state {
                    Some(root_state) if root_state.status == ProcessedStatus::Mined => {}
                    _ => {
                        tx.mark_root_as_mined(&root).await?;
                    }
                }

                Ok::<(), anyhow::Error>(())
            })
            .await?;

            info!(?root, "Root finalized");

            sync_tree_notify.notify_one();
        }

        Ok(())
    }

    fn extract_roots_from_mainnet_logs(mainnet_logs: Vec<Log>) -> Vec<Hash> {
        let mut roots = vec![];
        for log in mainnet_logs {
            let Some(event) = Self::raw_log_to_tree_changed(&log) else {
                continue;
            };

            let post_root = event.post_root;

            roots.push(post_root.into());
        }
        roots
    }

    fn raw_log_to_tree_changed(log: &Log) -> Option<TreeChangedFilter> {
        let raw_log = RawLog::from((log.topics.clone(), log.data.to_vec()));

        TreeChangedFilter::decode_log(&raw_log).ok()
    }

    fn extract_roots_from_secondary_logs(logs: &[Log]) -> Vec<Hash> {
        let mut roots = vec![];

        for log in logs {
            let raw_log = RawLog::from((log.topics.clone(), log.data.to_vec()));
            if let Ok(event) = RootAddedFilter::decode_log(&raw_log) {
                roots.push(event.root.into());
            }
        }

        roots
    }
}

pub struct OffChainIdentityProcessor {
    database: Arc<Database>,
}

#[async_trait]
impl IdentityProcessor for OffChainIdentityProcessor {
    async fn commit_identities(&self, batch: &BatchEntry) -> anyhow::Result<TransactionId> {
        Ok(batch.id.to_string())
    }

    async fn finalize_identities(&self, sync_tree_notify: &Arc<Notify>) -> anyhow::Result<()> {
        let mut tx = self
            .database
            .begin_tx(IsolationLevel::RepeatableRead)
            .await?;
        let batch = tx.get_latest_batch().await?;

        let Some(batch) = batch else {
            tx.commit().await?;
            return Ok(());
        };

        // With current flow it is required to mark root as processed first as this is
        // how required mined_at field is set, We set proper state only if not set
        // previously.
        let root_state = tx.get_root_state(&batch.next_root).await?;

        if root_state.is_none() {
            // If root is not in identities table we can't mark it as processed or mined.
            // It happens sometimes as we do not have atomic operation for database and tree
            // insertion.
            // TODO: check if this is still possible after HA being done
            tx.commit().await?;
            return Ok(());
        }

        match root_state {
            Some(root_state) if root_state.status == ProcessedStatus::Processed => {
                tx.mark_root_as_mined(&batch.next_root).await?;
            }
            Some(root_state) if root_state.status == ProcessedStatus::Mined => {}
            _ => {
                tx.mark_root_as_processed(&batch.next_root).await?;
                tx.mark_root_as_mined(&batch.next_root).await?;
            }
        }

        tx.commit().await?;
        sync_tree_notify.notify_one();

        Ok(())
    }

    async fn mine_transaction(&self, _transaction_id: TransactionId) -> anyhow::Result<bool> {
        // For off chain mode we don't mine transactions, so we treat all of them as
        // mined
        Ok(true)
    }

    async fn tree_init_correction(&self, _initial_root_hash: &Hash) -> anyhow::Result<()> {
        // For off chain mode we assume tree in database is always correct
        Ok(())
    }

    async fn latest_root(&self) -> anyhow::Result<Option<Hash>> {
        Ok(self
            .database
            .get_latest_root_by_status(ProcessedStatus::Mined)
            .await?)
    }
}

impl OffChainIdentityProcessor {
    pub async fn new(database: Arc<Database>) -> anyhow::Result<Self> {
        Ok(OffChainIdentityProcessor { database })
    }
}
