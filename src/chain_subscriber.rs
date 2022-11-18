use std::{sync::Arc, time::Duration};
use tokio::{task::JoinHandle, sync::RwLock, time::sleep};
use tracing::{instrument, info};
use crate::{database::Database, contracts::Contracts, app::{SharedTreeState}};

struct RunningInstance {
    #[allow(dead_code)]
    handle:         JoinHandle<eyre::Result<()>>,
}


pub struct ChainSubscriber {
    instance:   RwLock<Option<RunningInstance>>,
    database:   Arc<Database>,
    contracts:  Arc<Contracts>,
    tree_state: SharedTreeState,
}

impl ChainSubscriber {
    pub fn new(
        database: Arc<Database>,
        contracts: Arc<Contracts>,
        tree_state: SharedTreeState,
    ) -> Self {
        Self {
            instance: RwLock::new(None),
            database,
            contracts,
            tree_state,
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn start(&self) {
        let mut instance = self.instance.write().await;
        if instance.is_some() {
            info!("Chain Subscriber already running");
            return;
        }

        let database = self.database.clone();
        let tree_state = self.tree_state.clone();
        let contracts = self.contracts.clone();
        let handle = tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(60)).await;
            }
        });
        *instance = Some(RunningInstance {
            handle,
        });
    }
}