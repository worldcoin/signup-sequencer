use std::sync::Arc;
use std::time::Duration;

use anyhow::Result as AnyhowResult;
use ethers::types::U256;
use tracing::{info, instrument};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{Canonical, TreeVersion, TreeWithNextVersion};
use crate::utils::async_queue::{AsyncPopGuard, AsyncQueue};

// It can take up to 40 minutes to bridge the root
const FINALIZE_ROOT_SLEEP_TIME: Duration = Duration::from_secs(30);

const MAX_FINALIZATION_ATTEMPTS: usize = 100;

pub struct FinalizeRoots {
    database:          Arc<Database>,
    identity_manager:  SharedIdentityManager,
    finalized_tree:    TreeVersion<Canonical>,
    mined_roots_queue: AsyncQueue<U256>,
}

impl FinalizeRoots {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        finalized_tree: TreeVersion<Canonical>,
        mined_roots_queue: AsyncQueue<U256>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            finalized_tree,
            mined_roots_queue,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        finalize_roots_loop(
            &self.database,
            &self.identity_manager,
            &self.finalized_tree,
            &self.mined_roots_queue,
        )
        .await
    }
}

async fn finalize_roots_loop(
    database: &Database,
    identity_manager: &IdentityManager,
    finalized_tree: &TreeVersion<Canonical>,
    mined_roots_queue: &AsyncQueue<U256>,
) -> AnyhowResult<()> {
    loop {
        let mined_root = mined_roots_queue.pop().await;

        finalize_root(&mined_root, database, identity_manager, finalized_tree).await?;

        mined_root.commit().await;
    }
}

#[instrument(level = "info", skip_all)]
async fn finalize_root(
    mined_root: &AsyncPopGuard<'_, U256>,
    database: &Database,
    identity_manager: &IdentityManager,
    finalized_tree: &TreeVersion<Canonical>,
) -> AnyhowResult<()> {
    let root = mined_root.read().await;

    info!(?root, "Finalizing root");

    let mut num_attempts = 0;
    loop {
        let is_root_finalized = identity_manager.is_root_mined_multi_chain(root).await?;

        if is_root_finalized {
            break;
        }

        num_attempts += 1;

        if num_attempts > MAX_FINALIZATION_ATTEMPTS {
            anyhow::bail!("Root {root} not finalized after {num_attempts} attempts, giving up",);
        }

        tokio::time::sleep(FINALIZE_ROOT_SLEEP_TIME).await;
    }

    finalized_tree.apply_updates_up_to(root.into());
    database.mark_root_as_mined(&root.into()).await?;

    info!(?root, "Root finalized");

    Ok(())
}
