use std::sync::Arc;
use std::time::Duration;

use futures::stream::FuturesUnordered;
use futures::StreamExt;
use once_cell::sync::Lazy;
use prometheus::{linear_buckets, register_gauge, register_histogram, Gauge, Histogram};
use tokio::select;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::{error, info, instrument, warn};

use crate::app::App;
use crate::database::methods::DbMethods as _;
use crate::database::Database;
use crate::shutdown::Shutdown;

pub mod tasks;

const TREE_INIT_BACKOFF: Duration = Duration::from_secs(5);
const PROCESS_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const FINALIZE_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const QUEUE_MONITOR_BACKOFF: Duration = Duration::from_secs(5);
const INSERT_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);
const DELETE_IDENTITIES_BACKOFF: Duration = Duration::from_secs(5);

static PENDING_IDENTITIES: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!("pending_identities", "Identities not submitted on-chain").unwrap()
});

static UNPROCESSED_IDENTITIES: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!(
        "unprocessed_identities",
        "Identities not processed by identity committer"
    )
    .unwrap()
});

static BATCH_SIZES: Lazy<Histogram> = Lazy::new(|| {
    register_histogram!(
        "submitted_batch_sizes",
        "Submitted batch size",
        linear_buckets(f64::from(1), f64::from(1), 100).unwrap()
    )
    .unwrap()
});

/// A task manager for all long running tasks
///
/// It's assumed that there is only one instance at a time.
/// Spawning multiple `TaskManagers` will result in undefined behavior,
/// including data duplication.
pub struct TaskMonitor;

impl TaskMonitor {
    /// Initialize and run the task monitor
    #[instrument(level = "debug", skip_all)]
    pub async fn init(main_app: Arc<App>, shutdown: Shutdown) {
        let (monitored_txs_sender, monitored_txs_receiver) =
            mpsc::channel(main_app.clone().config.app.monitored_txs_capacity);

        let monitored_txs_sender = Arc::new(monitored_txs_sender);
        let monitored_txs_receiver = Arc::new(Mutex::new(monitored_txs_receiver));

        let base_wake_up_notify = Arc::new(Notify::new());
        // Immediately notify so we can start processing if we have pending identities
        // in the database
        base_wake_up_notify.notify_one();

        let handles = FuturesUnordered::new();

        // Initialize the Tree
        let app = main_app.clone();
        let tree_init = move || app.clone().init_tree();
        let tree_init_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            tree_init,
            TREE_INIT_BACKOFF,
            shutdown.clone(),
        );

        handles.push(tree_init_handle);

        // Finalize identities
        let app = main_app.clone();
        let finalize_identities = move || tasks::finalize_identities::finalize_roots(app.clone());
        let finalize_identities_handle = crate::utils::spawn_with_backoff(
            finalize_identities,
            FINALIZE_IDENTITIES_BACKOFF,
            shutdown.clone(),
        );
        handles.push(finalize_identities_handle);

        // Report length of the queue of identities
        let app = main_app.clone();
        let queue_monitor = move || tasks::monitor_queue::monitor_queue(app.clone());
        let queue_monitor_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            queue_monitor,
            QUEUE_MONITOR_BACKOFF,
            shutdown.clone(),
        );
        handles.push(queue_monitor_handle);

        // Process identities
        let base_next_batch_notify = Arc::new(Notify::new());

        // Create batches
        let app = main_app.clone();
        let next_batch_notify = base_next_batch_notify.clone();
        let wake_up_notify = base_wake_up_notify.clone();

        let create_batches = move || {
            tasks::create_batches::create_batches(
                app.clone(),
                next_batch_notify.clone(),
                wake_up_notify.clone(),
            )
        };
        let create_batches_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            create_batches,
            PROCESS_IDENTITIES_BACKOFF,
            shutdown.clone(),
        );
        handles.push(create_batches_handle);

        // Process batches
        let app = main_app.clone();
        let next_batch_notify = base_next_batch_notify.clone();
        let wake_up_notify = base_wake_up_notify.clone();

        let process_identities = move || {
            tasks::process_batches::process_batches(
                app.clone(),
                monitored_txs_sender.clone(),
                next_batch_notify.clone(),
                wake_up_notify.clone(),
            )
        };
        let process_identities_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            process_identities,
            PROCESS_IDENTITIES_BACKOFF,
            shutdown.clone(),
        );
        handles.push(process_identities_handle);

        // Monitor transactions
        let app = main_app.clone();
        let monitor_txs =
            move || tasks::monitor_txs::monitor_txs(app.clone(), monitored_txs_receiver.clone());
        let monitor_txs_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            monitor_txs,
            PROCESS_IDENTITIES_BACKOFF,
            shutdown.clone(),
        );
        handles.push(monitor_txs_handle);

        let pending_insertion_mutex = Arc::new(Mutex::new(()));

        // Insert identities
        let app = main_app.clone();
        let wake_up_notify = base_wake_up_notify.clone();
        let insertion_mutex = pending_insertion_mutex.clone();
        let insert_identities = move || {
            self::tasks::insert_identities::insert_identities(
                app.clone(),
                insertion_mutex.clone(),
                wake_up_notify.clone(),
            )
        };

        let insert_identities_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            insert_identities,
            INSERT_IDENTITIES_BACKOFF,
            shutdown.clone(),
        );
        handles.push(insert_identities_handle);

        // Delete identities
        let app = main_app.clone();
        let wake_up_notify = base_wake_up_notify.clone();
        let delete_identities = move || {
            self::tasks::delete_identities::delete_identities(
                app.clone(),
                pending_insertion_mutex.clone(),
                wake_up_notify.clone(),
            )
        };

        let delete_identities_handle = crate::utils::spawn_with_backoff_cancel_on_shutdown(
            delete_identities,
            DELETE_IDENTITIES_BACKOFF,
            shutdown.clone(),
        );
        handles.push(delete_identities_handle);

        tokio::spawn(Self::monitor_shutdown(handles, shutdown.clone()));
    }

    async fn monitor_shutdown(mut handles: FuturesUnordered<JoinHandle<()>>, shutdown: Shutdown) {
        select! {
            // Wait for the shutdown signal
            _ = shutdown.await_shutdown_begin() => {
             }
            // Or wait for a task to panic
            _ = Self::await_task_panic(&mut handles, shutdown.clone()) => {}
        };
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
        warn!("all tasks have returned unexpectedly");
    }

    async fn log_pending_identities_count(database: &Database) -> anyhow::Result<()> {
        let identities = database.count_pending_identities().await?;
        PENDING_IDENTITIES.set(f64::from(identities));
        Ok(())
    }

    async fn log_unprocessed_identities_count(database: &Database) -> anyhow::Result<()> {
        let identities = database.count_unprocessed_identities().await?;
        UNPROCESSED_IDENTITIES.set(f64::from(identities));
        Ok(())
    }

    async fn log_identities_queues(database: &Database) -> anyhow::Result<()> {
        TaskMonitor::log_unprocessed_identities_count(database).await?;
        TaskMonitor::log_pending_identities_count(database).await?;
        Ok(())
    }

    #[allow(clippy::cast_precision_loss)]
    fn log_batch_size(size: usize) {
        BATCH_SIZES.observe(size as f64);
    }
}
