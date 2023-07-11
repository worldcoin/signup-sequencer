use std::sync::Arc;

use anyhow::Result as AnyhowResult;
use ethers::types::U256;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, instrument, warn};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{Canonical, TreeVersion, TreeWithNextVersion};

pub struct FinalizeRoots {
    database:             Arc<Database>,
    identity_manager:     SharedIdentityManager,
    finalized_tree:       TreeVersion<Canonical>,
    mined_roots_receiver: Arc<Mutex<mpsc::Receiver<U256>>>,
}

impl FinalizeRoots {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        finalized_tree: TreeVersion<Canonical>,
        mined_roots_receiver: Arc<Mutex<mpsc::Receiver<U256>>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            finalized_tree,
            mined_roots_receiver,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        let mut mined_roots_receiver = self.mined_roots_receiver.lock().await;

        finalize_roots_loop(
            &self.database,
            &self.identity_manager,
            &self.finalized_tree,
            &mut mined_roots_receiver,
        )
        .await
    }
}

async fn finalize_roots_loop(
    database: &Database,
    identity_manager: &IdentityManager,
    finalized_tree: &TreeVersion<Canonical>,
    mined_roots_receiver: &mut mpsc::Receiver<U256>,
) -> AnyhowResult<()> {
    loop {
        let Some(mined_root) = mined_roots_receiver.recv().await else {
            warn!("Pending identities channel closed, terminating.");
            break;
        };

        finalize_root(mined_root, database, identity_manager, finalized_tree).await?;
    }

    Ok(())
}

#[instrument(level = "info", skip_all)]
async fn finalize_root(
    mined_root: U256,
    database: &Database,
    _identity_manager: &IdentityManager,
    finalized_tree: &TreeVersion<Canonical>,
) -> AnyhowResult<()> {
    info!(?mined_root, "Finalizing root");

    // TODO: implement

    finalized_tree.apply_updates_up_to(mined_root.into());

    // With this done, all that remains is to mark them as submitted to the
    // blockchain in the source-of-truth database, and also update the mined tree to
    // agree with the database and chain.
    database.mark_root_as_finalized(&mined_root.into()).await?;

    info!(?mined_root, "Root finalized");

    Ok(())
}
