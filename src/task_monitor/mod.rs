use std::sync::Arc;
use std::time::Duration;

use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::select;
use tokio::sync::{mpsc, watch, Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::{error, info, instrument, warn};

use crate::app::App;
use crate::shutdown::Shutdown;

pub mod tasks;

const CREATE_BATCHES_BACKOFF: Duration = Duration::from_secs(5);
const PROCESS_BATCHES_BACKOFF: Duration = Duration::from_secs(5);
const MONITOR_TXS_BACKOFF: Duration = Duration::from_secs(5);
const FINALIZE_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const QUEUE_MONITOR_BACKOFF: Duration = Duration::from_secs(5);
const MODIFY_TREE_BACKOFF: Duration = Duration::from_secs(5);
const SYNC_TREE_STATE_WITH_DB_BACKOFF: Duration = Duration::from_secs(5);

/// A task manager for all long running tasks
///
/// It's assumed that there is only one instance at a time.
/// Spawning multiple `TaskManagers` will result in undefined behavior,
/// including data duplication.
pub struct TaskMonitor;

impl TaskMonitor {
    /// Initialize and run the task monitor
    #[instrument(level = "debug", skip_all)]
    pub async fn init(main_app: Arc<App>, shutdown: Shutdown) -> anyhow::Result<()> {
        let (monitored_txs_sender, monitored_txs_receiver) =
            mpsc::channel(main_app.clone().config.app.monitored_txs_capacity);

        let monitored_txs_sender = Arc::new(monitored_txs_sender);
        let monitored_txs_receiver = Arc::new(Mutex::new(monitored_txs_receiver));

        let base_next_batch_notify = Arc::new(Notify::new());
        // Immediately notify, so we can start processing if we have pending operations
        base_next_batch_notify.notify_one();

        let base_sync_tree_notify = Arc::new(Notify::new());
        // Immediately notify, so we can start processing if we have pending operations
        base_sync_tree_notify.notify_one();

        let (base_tree_synced_tx, base_tree_synced_rx) = watch::channel(());
        // Immediately notify, so we can start processing if we have pending operations
        let _ = base_tree_synced_tx.send(());

        let handles = FuturesUnordered::new();

        // Initialize the Tree
        let app = main_app.clone();
        app.init_tree().await?;

        // Finalize identities
        let app = main_app.clone();
        let sync_tree_notify = base_sync_tree_notify.clone();

        let finalize_identities = move || {
            tasks::finalize_identities::finalize_roots(app.clone(), sync_tree_notify.clone())
        };
        let finalize_identities_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            "finalize_identities".to_string(),
            finalize_identities,
            FINALIZE_IDENTITIES_BACKOFF,
            shutdown.clone(),
        );
        handles.push(finalize_identities_handle);

        // Report length of the queue of identities
        let app = main_app.clone();
        let queue_monitor = move || tasks::monitor_queue::monitor_queue(app.clone());
        let queue_monitor_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            "queue_monitor".to_string(),
            queue_monitor,
            QUEUE_MONITOR_BACKOFF,
            shutdown.clone(),
        );
        handles.push(queue_monitor_handle);

        // Create batches
        let app = main_app.clone();
        let next_batch_notify = base_next_batch_notify.clone();
        let sync_tree_notify = base_sync_tree_notify.clone();
        let tree_synced_rx = base_tree_synced_rx.clone();

        let create_batches = move || {
            tasks::create_batches::create_batches(
                app.clone(),
                next_batch_notify.clone(),
                sync_tree_notify.clone(),
                tree_synced_rx.clone(),
            )
        };
        let create_batches_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            "create_batches".to_string(),
            create_batches,
            CREATE_BATCHES_BACKOFF,
            shutdown.clone(),
        );
        handles.push(create_batches_handle);

        // Process batches
        let app = main_app.clone();
        let next_batch_notify = base_next_batch_notify.clone();

        let process_batches = move || {
            tasks::process_batches::process_batches(
                app.clone(),
                monitored_txs_sender.clone(),
                next_batch_notify.clone(),
            )
        };
        let process_batches_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            "process_batches".to_string(),
            process_batches,
            PROCESS_BATCHES_BACKOFF,
            shutdown.clone(),
        );
        handles.push(process_batches_handle);

        // Monitor transactions
        let app = main_app.clone();
        let monitor_txs =
            move || tasks::monitor_txs::monitor_txs(app.clone(), monitored_txs_receiver.clone());
        let monitor_txs_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            "monitor_txs".to_string(),
            monitor_txs,
            MONITOR_TXS_BACKOFF,
            shutdown.clone(),
        );
        handles.push(monitor_txs_handle);

        // Modify tree
        let app = main_app.clone();
        let sync_tree_notify = base_sync_tree_notify.clone();
        let tree_synced_notify = base_tree_synced_rx.clone();

        let modify_tree = move || {
            tasks::modify_tree::modify_tree(
                app.clone(),
                sync_tree_notify.clone(),
                tree_synced_notify.clone(),
            )
        };
        let modify_tree_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            "modify_tree".to_string(),
            modify_tree,
            MODIFY_TREE_BACKOFF,
            shutdown.clone(),
        );
        handles.push(modify_tree_handle);

        // Sync tree state with DB
        let app = main_app.clone();
        let sync_tree_notify = base_sync_tree_notify.clone();
        let tree_synced_tx = base_tree_synced_tx.clone();

        let sync_tree_state_with_db = move || {
            tasks::sync_tree_state_with_db::sync_tree_state_with_db(
                app.clone(),
                sync_tree_notify.clone(),
                tree_synced_tx.clone(),
            )
        };
        let sync_tree_state_with_db_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            "sync_tree_state_with_db".to_string(),
            sync_tree_state_with_db,
            SYNC_TREE_STATE_WITH_DB_BACKOFF,
            shutdown.clone(),
        );
        handles.push(sync_tree_state_with_db_handle);

        tokio::spawn(Self::monitor_shutdown(handles, shutdown.clone()));

        Ok(())
    }

    async fn monitor_shutdown(mut handles: FuturesUnordered<JoinHandle<()>>, shutdown: Shutdown) {
        select! {
            // Wait for the shutdown signal
            _ = shutdown.await_shutdown_begin() => {
             }
            // Or wait for a task to panic
            _ = Self::await_task_panic(&mut handles, shutdown.clone()) => {}
        }
    }

    async fn await_task_panic(handles: &mut FuturesUnordered<JoinHandle<()>>, shutdown: Shutdown) {
        while let Some(result) = handles.next().await {
            if !shutdown.is_shutting_down() {
                match result {
                    Ok(_) => {
                        info!("task exited");
                    }
                    Err(error) => {
                        error!(?error, "task panicked");
                        // Instruct the rest of the app to shutdown
                        shutdown.clone().shutdown();
                        return;
                    }
                }
            }
        }
        if !shutdown.is_shutting_down() {
            warn!("all tasks have returned unexpectedly");
        }
    }
}
