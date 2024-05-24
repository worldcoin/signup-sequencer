use std::sync::Arc;

use tokio::time::{sleep, Duration};

use crate::task_monitor::{App, TaskMonitor};

// How often send metrics for idenity queue length
const QUEUE_MONITORING_PERIOD: Duration = Duration::from_secs(1);

pub async fn monitor_queue(app: Arc<App>) -> anyhow::Result<()> {
    loop {
        TaskMonitor::log_identities_queues(&app.database).await?;
        sleep(QUEUE_MONITORING_PERIOD).await;
    }
}
