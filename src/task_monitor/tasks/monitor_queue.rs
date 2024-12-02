use crate::task_monitor::{App, TaskMonitor};
use std::sync::Arc;
use tokio::time;
use tokio::time::{Duration, MissedTickBehavior};
use tracing::info;

// How often send metrics for identity queue length
const QUEUE_MONITORING_PERIOD: Duration = Duration::from_secs(30);

pub async fn monitor_queue(app: Arc<App>) -> anyhow::Result<()> {
    let mut timer = time::interval(QUEUE_MONITORING_PERIOD);
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        timer.tick().await;
        info!("Monitor queue woken due to timeout.");

        TaskMonitor::log_identities_queues(&app.database).await?;
    }
}
