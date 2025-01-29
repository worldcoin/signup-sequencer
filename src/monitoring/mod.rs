use crate::database::methods::DbMethods;
use crate::database::Database;
use once_cell::sync::Lazy;
use prometheus::{linear_buckets, register_gauge, register_histogram, Gauge, Histogram};

static UNPROCESSED_IDENTITIES: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!(
        "unprocessed_identities",
        "Identities not yet put to the tree"
    )
    .unwrap()
});

static PENDING_IDENTITIES: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!(
        "pending_identities",
        "Identities in the tree not yet mined on-chain"
    )
    .unwrap()
});

static PENDING_BATCHES: Lazy<Gauge> =
    Lazy::new(|| register_gauge!("pending_batches", "Batches with pending identities").unwrap());

static BATCH_SIZES: Lazy<Histogram> = Lazy::new(|| {
    register_histogram!(
        "submitted_batch_sizes",
        "Submitted batch size",
        linear_buckets(f64::from(1), f64::from(1), 100).unwrap()
    )
    .unwrap()
});

pub struct Monitoring;

impl Monitoring {
    pub async fn log_identities_queues(database: &Database) -> anyhow::Result<()> {
        Self::log_unprocessed_identities_count(database).await?;
        Self::log_pending_identities_count(database).await?;
        Self::log_pending_batches_count(database).await?;
        Ok(())
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn log_batch_size(size: usize) {
        BATCH_SIZES.observe(size as f64);
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

    async fn log_pending_batches_count(database: &Database) -> anyhow::Result<()> {
        let identities = database.count_not_finalized_batches().await?;
        PENDING_BATCHES.set(f64::from(identities));
        Ok(())
    }
}
