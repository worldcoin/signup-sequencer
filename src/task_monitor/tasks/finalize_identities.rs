use std::sync::Arc;
use std::time::Duration;

use anyhow::Result as AnyhowResult;
use ethers::types::U256;
use tracing::{info, instrument};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{Canonical, TreeVersion, TreeWithNextVersion};
use crate::utils::async_queue::{AsyncPopGuard, AsyncQueue};

pub struct FinalizeRoots {
    database:          Arc<Database>,
    identity_manager:  SharedIdentityManager,
    finalized_tree:    TreeVersion<Canonical>,

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

async fn finalize_roots_loop(
    database: &Database,
    identity_manager: &IdentityManager,
    finalized_tree: &TreeVersion<Canonical>,
    finalization_max_attempts: usize,
    finalization_sleep_time: Duration,
) -> AnyhowResult<()> {
    loop {
        finalize_root(
            database,
            identity_manager,
            finalized_tree,
            finalization_max_attempts,
            finalization_sleep_time,
        )
        .await?;
    }
}

#[instrument(level = "info", skip_all)]
async fn finalize_root(
    database: &Database,
    identity_manager: &IdentityManager,
    finalized_tree: &TreeVersion<Canonical>,
    finalization_max_attempts: usize,
    finalization_sleep_time: Duration,
) -> AnyhowResult<()> {
    let root = todo!();

    info!(?root, "Finalizing root");

    let mut num_attempts = 0;
    loop {
        let is_root_finalized = identity_manager.is_root_mined_multi_chain(root).await?;

        if is_root_finalized {
            break;
        }

        num_attempts += 1;

        if num_attempts > finalization_max_attempts {
            anyhow::bail!("Root {root} not finalized after {num_attempts} attempts, giving up",);
        }

        tokio::time::sleep(finalization_sleep_time).await;
    }

    finalized_tree.apply_updates_up_to(root.into());
    database.mark_root_as_mined(&root.into()).await?;

    info!(?root, "Root finalized");

    Ok(())
}
