use chrono::{DateTime, Utc};
use sqlx::{Executor, Row};
use tracing::instrument;

use crate::database::query::DatabaseQuery;
use crate::database::types::CommitmentHistoryEntry;
use crate::database::{Database, Error};
use crate::identity_tree::{Hash, ProcessedStatus, UnprocessedStatus};

/// impl block for database transactions
impl Database {
    /// Marks the identities and roots from before a given root hash as mined
    /// Also marks following roots as pending
    #[instrument(skip(self), level = "debug")]
    pub async fn mark_root_as_processed(&self, root: &Hash) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;

        let root_id = tx.get_id_by_root(root).await?;

        let Some(root_id) = root_id else {
            return Err(Error::MissingRoot { root: *root });
        };

        let root_id = root_id as i64;
        // TODO: Can I get rid of line `AND    status <> $2
        let update_previous_roots = sqlx::query(
            r#"
            UPDATE identities
            SET    status = $2, mined_at = CURRENT_TIMESTAMP
            WHERE  id <= $1
            AND    status <> $2
            AND    status <> $3;
            "#,
        )
        .bind(root_id)
        .bind(<&str>::from(ProcessedStatus::Processed))
        .bind(<&str>::from(ProcessedStatus::Mined));

        let update_next_roots = sqlx::query(
            r#"
            UPDATE identities
            SET    status = $2, mined_at = NULL
            WHERE  id > $1
            "#,
        )
        .bind(root_id)
        .bind(<&str>::from(ProcessedStatus::Pending));

        tx.execute(update_previous_roots).await?;
        tx.execute(update_next_roots).await?;

        tx.commit().await?;

        Ok(())
    }

    /// Marks the identities and roots from before a given root hash as
    /// finalized
    #[instrument(skip(self), level = "debug")]
    pub async fn mark_root_as_mined(&self, root: &Hash) -> Result<(), Error> {
        let mined_status = ProcessedStatus::Mined;

        let mut tx = self.pool.begin().await?;
        tx.execute("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE;")
            .await?;

        let root_id = tx.get_id_by_root(root).await?;

        let Some(root_id) = root_id else {
            return Err(Error::MissingRoot { root: *root });
        };

        let root_id = root_id as i64;

        let update_previous_roots = sqlx::query(
            r#"
            UPDATE identities
            SET    status = $2
            WHERE  id <= $1
            AND    status <> $2
            "#,
        )
        .bind(root_id)
        .bind(<&str>::from(mined_status));

        tx.execute(update_previous_roots).await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn get_identity_history_entries(
        &self,
        commitment: &Hash,
    ) -> Result<Vec<CommitmentHistoryEntry>, Error> {
        let unprocessed = sqlx::query(
            r#"
            SELECT commitment, status, eligibility
            FROM unprocessed_identities
            WHERE commitment = $1
        "#,
        )
        .bind(commitment);

        let rows = self.pool.fetch_all(unprocessed).await?;
        let unprocessed_updates = rows
            .into_iter()
            .map(|row| {
                let eligibility_timestamp: DateTime<Utc> = row.get(2);
                let held_back = Utc::now() < eligibility_timestamp;

                CommitmentHistoryEntry {
                    leaf_index: None,
                    commitment: row.get::<Hash, _>(0),
                    held_back,
                    status: row
                        .get::<&str, _>(1)
                        .parse()
                        .expect("Failed to parse unprocessed status"),
                }
            })
            .collect::<Vec<CommitmentHistoryEntry>>();

        let leaf_index = self.get_identity_leaf_index(commitment).await?;
        let Some(leaf_index) = leaf_index else {
            return Ok(unprocessed_updates);
        };

        let identity_deletions = sqlx::query(
            r#"
            SELECT commitment
            FROM deletions
            WHERE leaf_index = $1
            "#,
        )
        .bind(leaf_index.leaf_index as i64);

        let rows = self.pool.fetch_all(identity_deletions).await?;
        let deletions = rows
            .into_iter()
            .map(|_row| CommitmentHistoryEntry {
                leaf_index: Some(leaf_index.leaf_index),
                commitment: Hash::ZERO,
                held_back:  false,
                status:     UnprocessedStatus::New.into(),
            })
            .collect::<Vec<CommitmentHistoryEntry>>();

        let processed_updates = sqlx::query(
            r#"
            SELECT commitment, status
            FROM identities
            WHERE leaf_index = $1
            ORDER BY id ASC
            "#,
        )
        .bind(leaf_index.leaf_index as i64);

        let rows = self.pool.fetch_all(processed_updates).await?;
        let processed_updates: Vec<CommitmentHistoryEntry> = rows
            .into_iter()
            .map(|row| CommitmentHistoryEntry {
                leaf_index: Some(leaf_index.leaf_index),
                commitment: row.get::<Hash, _>(0),
                held_back:  false,
                status:     row
                    .get::<&str, _>(1)
                    .parse()
                    .expect("Status is unreadable, database is corrupt"),
            })
            .collect();

        Ok([processed_updates, unprocessed_updates, deletions]
            .concat()
            .into_iter()
            .collect())
    }
}
