use std::collections::HashSet;

use axum::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Acquire, Executor, Postgres, Row};
use tracing::instrument;

use super::types::{
    DeletionEntry, LatestDeletionEntry, LatestInsertionEntry, UnprocessedIdentityEntry,
};
use crate::database::types::{BatchEntry, BatchEntryData, BatchType};
use crate::database::Error;
use crate::identity_tree::{Hash, ProcessedStatus, RootItem, TreeItem, TreeUpdate};
use crate::prover::identity::Identity;
use crate::prover::{ProverConfig, ProverType};

const MAX_UNPROCESSED_FETCH_COUNT: i64 = 10_000;

#[async_trait]
pub trait DbMethods<'c>: Acquire<'c, Database = Postgres> + Sized {
    #[instrument(skip(self), level = "debug")]
    async fn insert_pending_identity(
        self,
        leaf_index: usize,
        identity: &Hash,
        received_at: Option<DateTime<Utc>>,
        root: &Hash,
        pre_root: &Hash,
    ) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            INSERT INTO identities (
                leaf_index,
                commitment,
                received_at,
                inserted_at,
                root,
                status,
                pending_as_of,
                pre_root
            )
            VALUES ($1, $2, $3, CURRENT_TIMESTAMP, $4, $5, CURRENT_TIMESTAMP, $6)
            "#,
        )
        .bind(leaf_index as i64)
        .bind(identity)
        .bind(received_at)
        .bind(root)
        .bind(<&str>::from(ProcessedStatus::Pending))
        .bind(pre_root)
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_id_by_root(self, root: &Hash) -> Result<Option<usize>, Error> {
        let mut conn = self.acquire().await?;

        let row = sqlx::query(
            r#"
            SELECT id
            FROM identities
            WHERE root = $1
            ORDER BY id ASC
            LIMIT 1
            "#,
        )
        .bind(root)
        .fetch_optional(&mut *conn)
        .await?;

        let Some(row) = row else { return Ok(None) };
        let root_id = row.get::<i64, _>(0);

        Ok(Some(root_id as usize))
    }

    /// Marks a root and associated entities as processed
    ///
    /// This is a composite operation performing multiple queries - it should be ran within a transaction.
    #[instrument(skip(self), level = "debug")]
    async fn mark_root_as_processed(self, root: &Hash) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        let root_id = conn.get_id_by_root(root).await?;

        let Some(root_id) = root_id else {
            return Err(Error::MissingRoot { root: *root });
        };

        let root_id = root_id as i64;

        sqlx::query(
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
        .bind(<&str>::from(ProcessedStatus::Mined))
        .execute(&mut *conn)
        .await?;

        sqlx::query(
            r#"
            UPDATE identities
            SET    status = $2, mined_at = NULL
            WHERE  id > $1
            "#,
        )
        .bind(root_id)
        .bind(<&str>::from(ProcessedStatus::Pending))
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    /// Marks a root and associated identities as mined
    ///
    /// This is a composite operation performing multiple queries - it should be ran within a transaction.
    #[instrument(skip(self), level = "debug")]
    async fn mark_root_as_mined(self, root: &Hash) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        let root_id = conn.get_id_by_root(root).await?;

        let Some(root_id) = root_id else {
            return Err(Error::MissingRoot { root: *root });
        };

        let root_id = root_id as i64;

        sqlx::query(
            r#"
            UPDATE identities
            SET    status = $2
            WHERE  id <= $1
            AND    status <> $2
            "#,
        )
        .bind(root_id)
        .bind(<&str>::from(ProcessedStatus::Mined))
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn mark_all_as_pending(self) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            UPDATE identities
            SET    status = $1, mined_at = NULL
            WHERE  status <> $1
            "#,
        )
        .bind(<&str>::from(ProcessedStatus::Pending))
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_next_leaf_index(self) -> Result<usize, Error> {
        let mut conn = self.acquire().await?;

        let row = sqlx::query(
            r#"
            SELECT leaf_index FROM identities
            ORDER BY leaf_index DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&mut *conn)
        .await?;

        let Some(row) = row else { return Ok(0) };
        let leaf_index = row.get::<i64, _>(0);

        Ok((leaf_index + 1) as usize)
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_tree_item(self, identity: &Hash) -> Result<Option<TreeItem>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT leaf_index, status, id as sequence_id, commitment as element
            FROM identities
            WHERE commitment = $1
            ORDER BY id DESC
            LIMIT 1;
            "#,
        )
        .bind(identity)
        .fetch_optional(&mut *conn)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_tree_item_by_leaf_index(
        self,
        leaf_index: usize,
    ) -> Result<Option<TreeItem>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT leaf_index, status, id as sequence_id, commitment as element
            FROM identities
            WHERE leaf_index = $1
            ORDER BY id DESC
            LIMIT 1;
            "#,
        )
        .bind(leaf_index as i64)
        .fetch_optional(&mut *conn)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_tree_updates_by_status(
        self,
        status: ProcessedStatus,
    ) -> Result<Vec<TreeUpdate>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT id as sequence_id, leaf_index, commitment as element, root as post_root, received_at
            FROM identities
            WHERE status = $1
            ORDER BY id ASC;
            "#,
        )
        .bind(<&str>::from(status))
        .fetch_all(&mut *conn)
        .await?)
    }

    async fn get_tree_updates_after_id(self, id: usize) -> Result<Vec<TreeUpdate>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT id as sequence_id, leaf_index, commitment as element, root as post_root, received_at
            FROM identities
            WHERE id > $1
            ORDER BY id ASC;
            "#,
        )
        .bind(id as i64)
        .fetch_all(&mut *conn)
        .await?)
    }

    async fn get_latest_tree_update_by_statuses(
        self,
        statuses: Vec<ProcessedStatus>,
    ) -> Result<Option<TreeUpdate>, Error> {
        let mut conn = self.acquire().await?;

        let statuses: Vec<&str> = statuses.into_iter().map(<&str>::from).collect();
        Ok(sqlx::query_as(
            r#"
            SELECT id as sequence_id, leaf_index, commitment as element, root as post_root, received_at
            FROM identities
            WHERE status = ANY($1)
            ORDER BY id DESC
            LIMIT 1;
            "#,
        )
        .bind(&statuses[..]) // Official workaround https://github.com/launchbadge/sqlx/blob/main/FAQ.md#how-can-i-do-a-select--where-foo-in--query
        .fetch_optional(&mut *conn)
        .await?)
    }

    async fn get_tree_update_by_root(self, root: &Hash) -> Result<Option<TreeUpdate>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT id as sequence_id, leaf_index, commitment as element, root as post_root, received_at
            FROM identities
            WHERE root = $1
            LIMIT 1;
            "#,
        )
        .bind(root) // Official workaround https://github.com/launchbadge/sqlx/blob/main/FAQ.md#how-can-i-do-a-select--where-foo-in--query
        .fetch_optional(&mut *conn)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_latest_root_by_status(
        self,
        status: ProcessedStatus,
    ) -> Result<Option<Hash>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query(
            r#"
            SELECT root FROM identities WHERE status = $1 ORDER BY id DESC LIMIT 1
            "#,
        )
        .bind(<&str>::from(status))
        .fetch_optional(&mut *conn)
        .await?
        .map(|r| r.get::<Hash, _>(0)))
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_root_state(self, root: &Hash) -> Result<Option<RootItem>, Error> {
        let mut conn = self.acquire().await?;

        // This tries really hard to do everything in one query to prevent race
        // conditions.
        Ok(sqlx::query_as(
            r#"
            SELECT
                root,
                status,
                pending_as_of as pending_valid_as_of,
                mined_at as mined_valid_as_of
            FROM identities
            WHERE root = $1
            ORDER BY id
            LIMIT 1
            "#,
        )
        .bind(root)
        .fetch_optional(&mut *conn)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_latest_insertion(self) -> Result<LatestInsertionEntry, Error> {
        let mut conn = self.acquire().await?;

        let row = sqlx::query(
            r#"
            SELECT insertion_timestamp
            FROM latest_insertion_timestamp
            WHERE Lock = 'X';"#,
        )
        .fetch_optional(&mut *conn)
        .await?;

        if let Some(row) = row {
            Ok(LatestInsertionEntry {
                timestamp: row.get(0),
            })
        } else {
            Ok(LatestInsertionEntry {
                timestamp: DateTime::from_timestamp_millis(1).unwrap(),
            })
        }
    }

    #[instrument(skip(self), level = "debug")]
    async fn count_unprocessed_identities(self) -> Result<i32, Error> {
        let mut conn = self.acquire().await?;

        let (count,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*) as unprocessed
            FROM unprocessed_identities
            "#,
        )
        .fetch_one(&mut *conn)
        .await?;

        Ok(count as i32)
    }

    #[instrument(skip(self), level = "debug")]
    async fn count_pending_identities(self) -> Result<i32, Error> {
        let mut conn = self.acquire().await?;

        let (count,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*) as pending
            FROM identities
            WHERE status = $1
            "#,
        )
        .bind(<&str>::from(ProcessedStatus::Pending))
        .fetch_one(&mut *conn)
        .await?;

        Ok(count as i32)
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_provers(self) -> Result<HashSet<ProverConfig>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT batch_size, url, timeout_s, prover_type
            FROM provers
            "#,
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect())
    }

    #[instrument(skip(self, url), level = "debug")]
    async fn insert_prover_configuration(
        self,
        batch_size: usize,
        url: impl ToString + Send,
        timeout_seconds: u64,
        prover_type: ProverType,
    ) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        let url = url.to_string();

        sqlx::query(
            r#"
            INSERT INTO provers (batch_size, url, timeout_s, prover_type)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(batch_size as i64)
        .bind(url)
        .bind(timeout_seconds as i64)
        .bind(prover_type)
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn insert_provers(self, provers: HashSet<ProverConfig>) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        if provers.is_empty() {
            return Ok(());
        }

        let mut query_builder = sqlx::QueryBuilder::new(
            r#"
            INSERT INTO provers (batch_size, url, timeout_s, prover_type)
            "#,
        );

        query_builder.push_values(provers, |mut b, prover| {
            b.push_bind(prover.batch_size as i64)
                .push_bind(prover.url)
                .push_bind(prover.timeout_s as i64)
                .push_bind(prover.prover_type);
        });

        let query = query_builder.build();

        conn.execute(query).await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn remove_prover(self, batch_size: usize, prover_type: ProverType) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            DELETE FROM provers WHERE batch_size = $1 AND prover_type = $2
            "#,
        )
        .bind(batch_size as i64)
        .bind(prover_type)
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn insert_unprocessed_identity(self, identity: Hash) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            INSERT INTO unprocessed_identities (commitment, created_at)
            VALUES ($1, CURRENT_TIMESTAMP)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(identity)
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_latest_deletion(self) -> Result<LatestDeletionEntry, Error> {
        let mut conn = self.acquire().await?;

        let row =
            sqlx::query("SELECT deletion_timestamp FROM latest_deletion_root WHERE Lock = 'X';")
                .fetch_optional(&mut *conn)
                .await?;

        if let Some(row) = row {
            Ok(LatestDeletionEntry {
                timestamp: row.get(0),
            })
        } else {
            Ok(LatestDeletionEntry {
                timestamp: DateTime::from_timestamp_millis(1).unwrap(),
            })
        }
    }

    #[instrument(skip(self), level = "debug")]
    async fn update_latest_insertion(
        self,
        insertion_timestamp: DateTime<Utc>,
    ) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            INSERT INTO latest_insertion_timestamp (Lock, insertion_timestamp)
            VALUES ('X', $1)
            ON CONFLICT (Lock)
            DO UPDATE SET insertion_timestamp = EXCLUDED.insertion_timestamp;
            "#,
        )
        .bind(insertion_timestamp)
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn update_latest_deletion(self, deletion_timestamp: DateTime<Utc>) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            INSERT INTO latest_deletion_root (Lock, deletion_timestamp)
            VALUES ('X', $1)
            ON CONFLICT (Lock)
            DO UPDATE SET deletion_timestamp = EXCLUDED.deletion_timestamp;
            "#,
        )
        .bind(deletion_timestamp)
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    /// Inserts a new deletion into the deletions table
    ///
    /// This method is idempotent and on conflict nothing will happen
    #[instrument(skip(self), level = "debug")]
    async fn insert_new_deletion(self, leaf_index: usize, identity: &Hash) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            INSERT INTO deletions (leaf_index, commitment, created_at)
            VALUES ($1, $2, CURRENT_TIMESTAMP)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(leaf_index as i64)
        .bind(identity)
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_deletion(self, commitment: &Hash) -> Result<Option<DeletionEntry>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT leaf_index, commitment, created_at
            FROM deletions
            WHERE commitment = $1
            "#,
        )
        .bind(commitment)
        .fetch_optional(&mut *conn)
        .await?)
    }

    // TODO: consider using a larger value than i64 for leaf index, ruint should
    // have postgres compatibility for u256
    #[instrument(skip(self), level = "debug")]
    async fn get_deletions(self) -> Result<Vec<DeletionEntry>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT leaf_index, commitment, created_at
            FROM deletions
            "#,
        )
        .fetch_all(&mut *conn)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    async fn count_deletions(self) -> Result<i64, Error> {
        let mut conn = self.acquire().await?;

        let (count,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*) as deletions
            FROM deletions
            "#,
        )
        .fetch_one(&mut *conn)
        .await?;

        Ok(count)
    }

    /// Remove a list of entries from the deletions table
    #[instrument(skip(self), level = "debug")]
    async fn remove_deletions(self, commitments: &[Hash]) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        let commitments = commitments
            .iter()
            .map(|c| c.to_be_bytes())
            .collect::<Vec<[u8; 32]>>();

        sqlx::query("DELETE FROM deletions WHERE commitment = Any($1)")
            .bind(commitments)
            .execute(&mut *conn)
            .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_unprocessed_identities(self) -> Result<Vec<UnprocessedIdentityEntry>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT commitment, created_at FROM unprocessed_identities
            LIMIT $1
            "#,
        )
        .bind(MAX_UNPROCESSED_FETCH_COUNT)
        .fetch_all(&mut *conn)
        .await?)
    }

    async fn get_unprocessed_commitment(self, commitment: &Hash) -> Result<Option<Hash>, Error> {
        let mut conn = self.acquire().await?;

        let result = sqlx::query(
            r#"
                SELECT commitment FROM unprocessed_identities WHERE commitment = $1
            "#,
        )
        .bind(commitment)
        .fetch_optional(&mut *conn)
        .await?;

        if let Some(row) = result {
            return Ok(Some(row.get::<Hash, _>(0)));
        };
        Ok(None)
    }

    #[instrument(skip(self), level = "debug")]
    async fn trim_unprocessed(self) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            DELETE FROM unprocessed_identities u
            USING identities i
            WHERE u.commitment = i.commitment
            "#,
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn identity_exists(self, commitment: Hash) -> Result<bool, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query(
            r#"
            select
            EXISTS (select commitment from unprocessed_identities where commitment = $1) OR
            EXISTS (select commitment from identities where commitment = $1);
            "#,
        )
        .bind(commitment)
        .fetch_one(&mut *conn)
        .await?
        .get::<bool, _>(0))
    }

    #[instrument(skip(self), level = "debug")]
    async fn insert_new_batch_head(self, next_root: &Hash) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            INSERT INTO batches(
                id,
                next_root,
                prev_root,
                created_at,
                batch_type,
                data
            ) VALUES (DEFAULT, $1, NULL, CURRENT_TIMESTAMP, $2, $3)
            "#,
        )
        .bind(next_root)
        .bind(BatchType::Insertion)
        .bind(sqlx::types::Json::from(BatchEntryData {
            identities: vec![],
            indexes: vec![],
        }))
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self, identities, indexes), level = "debug")]
    async fn insert_new_batch(
        self,
        next_root: &Hash,
        prev_root: &Hash,
        batch_type: BatchType,
        identities: &[Identity],
        indexes: &[usize],
    ) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            INSERT INTO batches(
                id,
                next_root,
                prev_root,
                created_at,
                batch_type,
                data
            ) VALUES (DEFAULT, $1, $2, CURRENT_TIMESTAMP, $3, $4)
            "#,
        )
        .bind(next_root)
        .bind(prev_root)
        .bind(batch_type)
        .bind(sqlx::types::Json::from(BatchEntryData {
            identities: identities.to_vec(),
            indexes: indexes.to_vec(),
        }))
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    #[cfg(test)]
    #[instrument(skip(self), level = "debug")]
    async fn get_next_batch(self, prev_root: &Hash) -> Result<Option<BatchEntry>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT
                id,
                next_root,
                prev_root,
                created_at,
                batch_type,
                data
            FROM batches WHERE prev_root = $1
            LIMIT 1
            "#,
        )
        .bind(prev_root)
        .fetch_optional(&mut *conn)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_latest_batch(self) -> Result<Option<BatchEntry>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT
                id,
                next_root,
                prev_root,
                created_at,
                batch_type,
                data
            FROM batches
            ORDER BY id DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&mut *conn)
        .await?)
    }

    async fn count_not_finalized_batches(self) -> Result<i32, Error> {
        let mut conn = self.acquire().await?;

        let res = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM batches
            LEFT JOIN transactions ON batches.next_root = transactions.batch_next_root
            LEFT JOIN identities ON batches.next_root = identities.root
            WHERE transactions.batch_next_root IS NOT NULL AND batches.prev_root IS NOT NULL AND identities.status = $1
            "#,
        )
        .bind(<&str>::from(ProcessedStatus::Pending))
        .fetch_one(&mut *conn)
        .await?;

        Ok(res.get::<i64, _>(0) as i32)
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_next_batch_without_transaction(self) -> Result<Option<BatchEntry>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT
                batches.id,
                batches.next_root,
                batches.prev_root,
                batches.created_at,
                batches.batch_type,
                batches.data
            FROM batches
            LEFT JOIN transactions ON batches.next_root = transactions.batch_next_root
            WHERE transactions.batch_next_root IS NULL AND batches.prev_root IS NOT NULL
            ORDER BY batches.id ASC
            LIMIT 1
            "#,
        )
        .fetch_optional(&mut *conn)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_batch_head(self) -> Result<Option<BatchEntry>, Error> {
        let mut conn = self.acquire().await?;

        Ok(sqlx::query_as(
            r#"
            SELECT
                id,
                next_root,
                prev_root,
                created_at,
                batch_type,
                data
            FROM batches WHERE prev_root IS NULL
            LIMIT 1
            "#,
        )
        .fetch_optional(&mut *conn)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    async fn insert_new_transaction(
        self,
        transaction_id: &String,
        batch_next_root: &Hash,
    ) -> Result<(), Error> {
        let mut conn = self.acquire().await?;

        sqlx::query(
            r#"
            INSERT INTO transactions(
                transaction_id,
                batch_next_root,
                created_at
            ) VALUES ($1, $2, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(transaction_id)
        .bind(batch_next_root)
        .execute(&mut *conn)
        .await?;

        Ok(())
    }
}

// Blanket implementation for all types that satisfy the trait bounds
impl<'c, T> DbMethods<'c> for T where T: Acquire<'c, Database = Postgres> + Send + Sync + Sized {}
