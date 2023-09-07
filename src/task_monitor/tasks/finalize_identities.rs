use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result as AnyhowResult;
use ethers::contract::Contract;
use ethers::providers::Middleware;
use ethers::types::{Address, Topic, ValueOrArray, U256};
use tracing::{info, instrument};

use crate::contracts::abi::{BridgedWorldId, RootAddedFilter, TreeChangedFilter, WorldId};
use crate::contracts::scanner::BlockScanner;
use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::ethereum::ReadProvider;
use crate::identity_tree::{Canonical, TreeVersion, TreeWithNextVersion};
use crate::utils::async_queue::{AsyncPopGuard, AsyncQueue};

pub struct FinalizeRoots {
    database:         Arc<Database>,
    identity_manager: SharedIdentityManager,
    finalized_tree:   TreeVersion<Canonical>,

    finalization_max_attempts: usize,
    finalization_sleep_time:   Duration,
}

impl FinalizeRoots {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        finalized_tree: TreeVersion<Canonical>,
        finalization_max_attempts: usize,
        finalization_sleep_time: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            finalized_tree,
            finalization_max_attempts,
            finalization_sleep_time,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        finalize_roots_loop(
            &self.database,
            &self.identity_manager,
            &self.finalized_tree,
            self.finalization_max_attempts,
            self.finalization_sleep_time,
        )
        .await
    }
}

const SCANNING_WINDOW_SIZE: u64 = 100;
const TIME_BETWEEN_SCANS: Duration = Duration::from_secs(5);

async fn finalize_roots_loop(
    database: &Database,
    identity_manager: &IdentityManager,
    finalized_tree: &TreeVersion<Canonical>,
    finalization_max_attempts: usize,
    finalization_sleep_time: Duration,
) -> AnyhowResult<()> {
    let mainnet_abi = identity_manager.abi();
    let secondary_abis = identity_manager.secondary_abis();

    let mut mainnet_scanner =
        BlockScanner::new_latest(mainnet_abi.client().clone(), SCANNING_WINDOW_SIZE).await?;
    let mut secondary_scanners = init_secondary_scanners(secondary_abis).await?;

    let mainnet_address = mainnet_abi.address();
    let mainnet_address = Some(ValueOrArray::Value(mainnet_address));

    use ethers::contract::EthEvent;

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

    loop {
        let mainnet_logs = mainnet_scanner
            .next(mainnet_address.clone(), mainnet_topics.clone())
            .await?;

        let mut secondary_logs = vec![];

        for (address, scanner) in &mut secondary_scanners {
            let logs = scanner
                .next(Some(ValueOrArray::Value(*address)), bridged_topics.clone())
                .await?;

            secondary_logs.extend(logs);
        }
    }
}

async fn init_secondary_scanners<T>(
    providers: &[BridgedWorldId<T>],
) -> anyhow::Result<HashMap<Address, BlockScanner<Arc<T>>>>
where
    T: Middleware,
    <T as Middleware>::Error: 'static,
{
    let mut secondary_scanners = HashMap::new();

    for bridged_abi in providers {
        let scanner =
            BlockScanner::new_latest(bridged_abi.client().clone(), SCANNING_WINDOW_SIZE).await?;

        let address = bridged_abi.address();

        secondary_scanners.insert(address, scanner);
    }

    Ok(secondary_scanners)
}

async fn fetch_logs() {}
