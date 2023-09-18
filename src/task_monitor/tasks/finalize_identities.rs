use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result as AnyhowResult;
use ethers::abi::RawLog;
use ethers::contract::EthEvent;
use ethers::providers::Middleware;
use ethers::types::{Address, Log, Topic, ValueOrArray, U256};
use tracing::{info, instrument};

use crate::contracts::abi::{BridgedWorldId, RootAddedFilter, TreeChangeKind, TreeChangedFilter};
use crate::contracts::scanner::BlockScanner;
use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{Canonical, Intermediate, TreeVersion, TreeWithNextVersion};

pub struct FinalizeRoots {
    database:         Arc<Database>,
    identity_manager: SharedIdentityManager,
    processed_tree:   TreeVersion<Intermediate>,
    finalized_tree:   TreeVersion<Canonical>,

    scanning_window_size: u64,
    time_between_scans:   Duration,
}

impl FinalizeRoots {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        processed_tree: TreeVersion<Intermediate>,
        finalized_tree: TreeVersion<Canonical>,
        scanning_window_size: u64,
        time_between_scans: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            processed_tree,
            finalized_tree,
            scanning_window_size,
            time_between_scans,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        finalize_roots_loop(
            &self.database,
            &self.identity_manager,
            &self.processed_tree,
            &self.finalized_tree,
            self.scanning_window_size,
            self.time_between_scans,
        )
        .await
    }
}

async fn finalize_roots_loop(
    database: &Database,
    identity_manager: &IdentityManager,
    processed_tree: &TreeVersion<Intermediate>,
    finalized_tree: &TreeVersion<Canonical>,
    scanning_window_size: u64,
    time_between_scans: Duration,
) -> AnyhowResult<()> {
    let mainnet_abi = identity_manager.abi();
    let secondary_abis = identity_manager.secondary_abis();

    let mut mainnet_scanner =
        BlockScanner::new_latest(mainnet_abi.client().clone(), scanning_window_size).await?;
    let mut secondary_scanners =
        init_secondary_scanners(secondary_abis, scanning_window_size).await?;

    let mainnet_address = mainnet_abi.address();

    loop {
        let mainnet_logs = fetch_mainnet_logs(&mut mainnet_scanner, mainnet_address).await?;

        finalize_mainnet_roots(database, identity_manager, processed_tree, mainnet_logs).await?;

        let secondary_logs = fetch_secondary_logs(&mut secondary_scanners).await?;

        finalize_secondary_roots(database, identity_manager, finalized_tree, secondary_logs)
            .await?;

        tokio::time::sleep(time_between_scans).await;
    }
}

#[instrument(level = "info", skip_all)]
async fn fetch_mainnet_logs<M>(
    mainnet_scanner: &mut BlockScanner<M>,
    mainnet_address: Address,
) -> anyhow::Result<Vec<TreeChangedFilter>>
where
    M: Middleware,
    <M as Middleware>::Error: 'static,
{
    let mainnet_topics = [
        Some(Topic::from(TreeChangedFilter::signature())),
        None,
        None,
        None,
    ];

    let mainnet_address = Some(ValueOrArray::Value(mainnet_address));

    let mainnet_logs = mainnet_scanner
        .next(mainnet_address, mainnet_topics.clone())
        .await?;

    Ok(extract_root_from_mainnet_logs(&mainnet_logs))
}

#[instrument(level = "info", skip_all)]
async fn fetch_secondary_logs<M>(
    secondary_scanners: &mut HashMap<Address, BlockScanner<M>>,
) -> anyhow::Result<Vec<U256>>
where
    M: Middleware,
    <M as Middleware>::Error: 'static,
{
    let bridged_topics = [
        Some(Topic::from(RootAddedFilter::signature())),
        None,
        None,
        None,
    ];

    let mut secondary_logs = vec![];

    for (address, scanner) in secondary_scanners {
        let logs = scanner
            .next(Some(ValueOrArray::Value(*address)), bridged_topics.clone())
            .await?;

        secondary_logs.extend(logs);
    }

    let roots = extract_roots_from_secondary_logs(&secondary_logs);

    Ok(roots)
}

#[instrument(level = "info", skip_all)]
async fn finalize_mainnet_roots(
    database: &Database,
    identity_manager: &IdentityManager,
    processed_tree: &TreeVersion<Intermediate>,
    all_roots: Vec<TreeChangedFilter>,
) -> Result<(), anyhow::Error> {
    for event in all_roots {
        let pre_root = event.pre_root;
        let post_root = event.post_root;

        info!(?pre_root, ?post_root, "Mining batch");

        // Double check
        if !identity_manager.is_root_mined(post_root).await? {
            continue;
        }

        database.mark_root_as_processed(&post_root.into()).await?;

        info!(?pre_root, ?post_root, "Batch mined");

        let updates_count = processed_tree.apply_updates_up_to(post_root.into());

        info!(updates_count, ?pre_root, ?post_root, "Mined tree updated");
    }

    Ok(())
}

#[instrument(level = "info", skip_all)]
async fn finalize_secondary_roots(
    database: &Database,
    identity_manager: &IdentityManager,
    finalized_tree: &TreeVersion<Canonical>,
    roots: Vec<U256>,
) -> Result<(), anyhow::Error> {
    for root in roots {
        info!(?root, "Finalizing root");

        // Check if mined on all L2s
        if !identity_manager.is_root_mined_multi_chain(root).await? {
            continue;
        }

        database.mark_root_as_mined(&root.into()).await?;
        finalized_tree.apply_updates_up_to(root.into());

        info!(?root, "Root finalized");
    }

    Ok(())
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
            BlockScanner::new_latest(bridged_abi.client().clone(), scanning_window_size).await?;

        let address = bridged_abi.address();

        secondary_scanners.insert(address, scanner);
    }

    Ok(secondary_scanners)
}

fn extract_root_from_mainnet_logs(logs: &[Log]) -> Vec<TreeChangedFilter> {
    let mut roots = vec![];

    for log in logs {
        let raw_log = RawLog::from((log.topics.clone(), log.data.to_vec()));
        if let Ok(event) = TreeChangedFilter::decode_log(&raw_log) {
            roots.push(event);
        }
    }

    roots
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
