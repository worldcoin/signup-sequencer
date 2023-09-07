use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result as AnyhowResult;
use ethers::abi::RawLog;
use ethers::contract::EthEvent;
use ethers::providers::Middleware;
use ethers::types::{Address, Log, Topic, ValueOrArray, H256, U256};
use tracing::{info, instrument};

use crate::contracts::abi::{BridgedWorldId, RootAddedFilter, TreeChangedFilter, WorldId};
use crate::contracts::scanner::BlockScanner;
use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{Canonical, TreeVersion, TreeWithNextVersion};

pub struct FinalizeRoots {
    database:         Arc<Database>,
    identity_manager: SharedIdentityManager,
    finalized_tree:   TreeVersion<Canonical>,

    scanning_window_size: u64,
    time_between_scans:   Duration,
}

impl FinalizeRoots {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        finalized_tree: TreeVersion<Canonical>,
        scanning_window_size: u64,
        time_between_scans: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            finalized_tree,
            scanning_window_size,
            time_between_scans,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        finalize_roots_loop(
            &self.database,
            &self.identity_manager,
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
        let all_roots = fetch_logs(
            &mut mainnet_scanner,
            &mut secondary_scanners,
            mainnet_address,
        )
        .await?;

        finalize_roots(database, identity_manager, finalized_tree, all_roots).await?;

        tokio::time::sleep(time_between_scans).await;
    }
}

async fn finalize_roots(
    database: &Database,
    identity_manager: &IdentityManager,
    finalized_tree: &TreeVersion<Canonical>,
    all_roots: Vec<U256>,
) -> Result<(), anyhow::Error> {
    Ok(for root in all_roots {
        info!(?root, "Finalizing root");

        let is_root_finalized = identity_manager.is_root_mined_multi_chain(root).await?;

        if is_root_finalized {
            finalized_tree.apply_updates_up_to(root.into());
            database.mark_root_as_mined(&root.into()).await?;

            info!(?root, "Root finalized");
        }
    })
}

async fn fetch_logs<A, B>(
    mainnet_scanner: &mut BlockScanner<A>,
    secondary_scanners: &mut HashMap<Address, BlockScanner<B>>,
    mainnet_address: Address,
) -> anyhow::Result<Vec<U256>>
where
    A: Middleware,
    <A as Middleware>::Error: 'static,
    B: Middleware,
    <B as Middleware>::Error: 'static,
{
    let mainnet_topics = [
        Some(Topic::from(TreeChangedFilter::signature())),
        None,
        None,
        None,
    ];

    let bridged_topics = [
        Some(Topic::from(RootAddedFilter::signature())),
        None,
        None,
        None,
    ];

    let mainnet_address = Some(ValueOrArray::Value(mainnet_address));

    let mainnet_logs = mainnet_scanner
        .next(mainnet_address, mainnet_topics.clone())
        .await?;
    let mut secondary_logs = vec![];

    for (address, scanner) in secondary_scanners {
        let logs = scanner
            .next(Some(ValueOrArray::Value(*address)), bridged_topics.clone())
            .await?;

        secondary_logs.extend(logs);
    }

    let mut all_roots = vec![];

    let mainnet_roots = extract_root_from_mainnet_logs(&mainnet_logs);
    let secondary_roots = extract_roots_from_secondary_logs(&secondary_logs);

    all_roots.extend(mainnet_roots);
    all_roots.extend(secondary_roots);

    Ok(all_roots)
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

fn extract_root_from_mainnet_logs(logs: &[Log]) -> Vec<U256> {
    let mut roots = vec![];

    for log in logs {
        let raw_log = RawLog::from((log.topics.clone(), log.data.to_vec()));
        if let Ok(event) = TreeChangedFilter::decode_log(&raw_log) {
            roots.push(event.post_root);
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
