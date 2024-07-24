use std::collections::HashSet;

use chrono::{DateTime, Utc};
use ruint::aliases::U256;
use sqlx::{Executor, Postgres, Row};
use tracing::instrument;
use types::{DeletionEntry, LatestDeletionEntry, RecoveryEntry};

use crate::database::types::{
    BatchEntry, BatchEntryData, BatchType, LatestInsertionEntry, TransactionEntry,
};
use crate::database::{types, Error};
use crate::identity_tree::{
    Hash, ProcessedStatus, RootItem, TreeItem, TreeUpdate, UnprocessedStatus,
};
use crate::prover::identity::Identity;
use crate::prover::{ProverConfig, ProverType};

const MAX_UNPROCESSED_FETCH_COUNT: i64 = 10_000;

/// This trait provides the individual and composable queries to the database.
/// Each method is a single atomic query, and can be composed within a
/// transaction.
pub trait DatabaseQuery<'a>: Executor<'a, Database = Postgres> {
    async fn insert_pending_identity(
        self,
        leaf_index: usize,
        identity: &Hash,
        root: &Hash,
        pre_root: &Hash,
    ) -> Result<(), Error> {
        let insert_pending_identity_query = sqlx::query(
            r#"
            INSERT INTO identities (leaf_index, commitment, root, status, pending_as_of, pre_root)
            VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP, $5)
            "#,
        )
        .bind(leaf_index as i64)
        .bind(identity)
        .bind(root)
        .bind(<&str>::from(ProcessedStatus::Pending))
        .bind(pre_root);

        self.execute(insert_pending_identity_query).await?;

        Ok(())
    }

    async fn get_id_by_root(self, root: &Hash) -> Result<Option<usize>, Error> {
        let root_index_query = sqlx::query(
            r#"
            SELECT id
            FROM identities
            WHERE root = $1
            ORDER BY id ASC
            LIMIT 1
            "#,
        )
        .bind(root);

        let row = self.fetch_optional(root_index_query).await?;

        let Some(row) = row else { return Ok(None) };
        let root_id = row.get::<i64, _>(0);

        Ok(Some(root_id as usize))
    }

    /// Marks all the identities in the db as
    #[instrument(skip(self), level = "debug")]
    async fn mark_all_as_pending(self) -> Result<(), Error> {
        let pending_status = ProcessedStatus::Pending;

        let update_all_identities = sqlx::query(
            r#"
                UPDATE identities
                SET    status = $1, mined_at = NULL
                WHERE  status <> $1
                "#,
        )
        .bind(<&str>::from(pending_status));

        self.execute(update_all_identities).await?;

        Ok(())
    }

    async fn get_next_leaf_index(self) -> Result<usize, Error> {
        let query = sqlx::query(
            r#"
            SELECT leaf_index FROM identities
            ORDER BY leaf_index DESC
            LIMIT 1
            "#,
        );

        let row = self.fetch_optional(query).await?;

        let Some(row) = row else { return Ok(0) };
        let leaf_index = row.get::<i64, _>(0);

        Ok((leaf_index + 1) as usize)
    }

    async fn get_identity_leaf_index(self, identity: &Hash) -> Result<Option<TreeItem>, Error> {
        let query = sqlx::query(
            r#"
            SELECT leaf_index, status
            FROM identities
            WHERE commitment = $1
            ORDER BY id DESC
            LIMIT 1;
            "#,
        )
        .bind(identity);

        let Some(row) = self.fetch_optional(query).await? else {
            return Ok(None);
        };

        let leaf_index = row.get::<i64, _>(0) as usize;

        let status = row
            .get::<&str, _>(1)
            .parse()
            .expect("Status is unreadable, database is corrupt");

        Ok(Some(TreeItem { status, leaf_index }))
    }

    async fn get_commitments_by_status(
        self,
        status: ProcessedStatus,
    ) -> Result<Vec<TreeUpdate>, Error> {
        Ok(sqlx::query_as::<_, TreeUpdate>(
            r#"
            SELECT id as sequence_id, leaf_index, commitment as element, root as post_root
            FROM identities
            WHERE status = $1
            ORDER BY id ASC;
            "#,
        )
        .bind(<&str>::from(status))
        .fetch_all(self)
        .await?)
    }

    async fn get_commitments_by_statuses(
        self,
        statuses: Vec<ProcessedStatus>,
    ) -> Result<Vec<TreeUpdate>, Error> {
        let statuses: Vec<&str> = statuses.into_iter().map(<&str>::from).collect();
        Ok(sqlx::query_as::<_, TreeUpdate>(
            r#"
            SELECT id as sequence_id, leaf_index, commitment as element, root as post_root
            FROM identities
            WHERE status = ANY($1)
            ORDER BY id ASC;
            "#,
        )
        .bind(&statuses[..]) // Official workaround https://github.com/launchbadge/sqlx/blob/main/FAQ.md#how-can-i-do-a-select--where-foo-in--query
        .fetch_all(self)
        .await?)
    }

    async fn get_commitments_after_id(self, id: usize) -> Result<Vec<TreeUpdate>, Error> {
        Ok(sqlx::query_as::<_, TreeUpdate>(
            r#"
            SELECT id as sequence_id, leaf_index, commitment as element, root as post_root
            FROM identities
            WHERE id > $1
            ORDER BY id ASC;
            "#,
        )
        .bind(id as i64)
        .fetch_all(self)
        .await?)
    }

    async fn get_non_zero_commitments_by_leaf_indexes<I: IntoIterator<Item = usize>>(
        self,
        leaf_indexes: I,
    ) -> Result<Vec<Hash>, Error> {
        let leaf_indexes: Vec<i64> = leaf_indexes.into_iter().map(|v| v as i64).collect();

        Ok(sqlx::query(
            r#"
            SELECT commitment
            FROM identities
            WHERE leaf_index = ANY($1)
            AND commitment != $2
            "#,
        )
        .bind(&leaf_indexes[..]) // Official workaround https://github.com/launchbadge/sqlx/blob/main/FAQ.md#how-can-i-do-a-select--where-foo-in--query
        .bind(Hash::ZERO)
        .fetch_all(self)
        .await?
        .into_iter()
        .map(|row| row.get::<Hash, _>(0))
        .collect())
    }

    async fn get_latest_root(self) -> Result<Option<Hash>, Error> {
        Ok(sqlx::query(
            r#"
            SELECT root FROM identities ORDER BY id DESC LIMIT 1
            "#,
        )
        .fetch_optional(self)
        .await?
        .map(|r| r.get::<Hash, _>(0)))
    }

    async fn get_latest_root_by_status(
        self,
        status: ProcessedStatus,
    ) -> Result<Option<Hash>, Error> {
        Ok(sqlx::query(
            r#"
            SELECT root FROM identities WHERE status = $1 ORDER BY id DESC LIMIT 1
            "#,
        )
        .bind(<&str>::from(status))
        .fetch_optional(self)
        .await?
        .map(|r| r.get::<Hash, _>(0)))
    }

    async fn get_root_state(self, root: &Hash) -> Result<Option<RootItem>, Error> {
        // This tries really hard to do everything in one query to prevent race
        // conditions.
        Ok(sqlx::query_as::<_, RootItem>(
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
        .fetch_optional(self)
        .await?)
    }

    async fn get_latest_tree_update_by_statuses(
        self,
        statuses: Vec<ProcessedStatus>,
    ) -> Result<Option<TreeUpdate>, Error> {
        let statuses: Vec<&str> = statuses.into_iter().map(<&str>::from).collect();
        Ok(sqlx::query_as::<_, TreeUpdate>(
            r#"
            SELECT id as sequence_id, leaf_index, commitment as element, root as post_root
            FROM identities
            WHERE status = ANY($1)
            ORDER BY id DESC
            LIMIT 1;
            "#,
        )
        .bind(&statuses[..]) // Official workaround https://github.com/launchbadge/sqlx/blob/main/FAQ.md#how-can-i-do-a-select--where-foo-in--query
        .fetch_optional(self)
        .await?)
    }

    async fn get_tree_update_by_root(self, root: &Hash) -> Result<Option<TreeUpdate>, Error> {
        Ok(sqlx::query_as::<_, TreeUpdate>(
            r#"
            SELECT id as sequence_id, leaf_index, commitment as element, root as post_root
            FROM identities
            WHERE root = $1
            LIMIT 1;
            "#,
        )
        .bind(root) // Official workaround https://github.com/launchbadge/sqlx/blob/main/FAQ.md#how-can-i-do-a-select--where-foo-in--query
        .fetch_optional(self)
        .await?)
    }

    async fn get_latest_insertion(self) -> Result<LatestInsertionEntry, Error> {
        let query = sqlx::query(
            r#"
            SELECT insertion_timestamp
            FROM latest_insertion_timestamp
            WHERE Lock = 'X';"#,
        );

        let row = self.fetch_optional(query).await?;

        if let Some(row) = row {
            Ok(LatestInsertionEntry {
                timestamp: row.get(0),
            })
        } else {
            Ok(LatestInsertionEntry {
                timestamp: Utc::now(),
            })
        }
    }

    async fn count_unprocessed_identities(self) -> Result<i32, Error> {
        let query = sqlx::query(
            r#"
            SELECT COUNT(*) as unprocessed
            FROM unprocessed_identities
            "#,
        );
        let result = self.fetch_one(query).await?;
        Ok(result.get::<i64, _>(0) as i32)
    }

    async fn count_pending_identities(self) -> Result<i32, Error> {
        let query = sqlx::query(
            r#"
            SELECT COUNT(*) as pending
            FROM identities
            WHERE status = $1
            "#,
        )
        .bind(<&str>::from(ProcessedStatus::Pending));
        let result = self.fetch_one(query).await?;
        Ok(result.get::<i64, _>(0) as i32)
    }

    async fn get_provers(self) -> Result<HashSet<ProverConfig>, Error> {
        Ok(sqlx::query_as(
            r#"
            SELECT batch_size, url, timeout_s, prover_type
            FROM provers
            "#,
        )
        .fetch_all(self)
        .await?
        .into_iter()
        .collect())
    }
    async fn insert_prover_configuration(
        self,
        batch_size: usize,
        url: impl ToString,
        timeout_seconds: u64,
        prover_type: ProverType,
    ) -> Result<(), Error> {
        let url = url.to_string();

        let query = sqlx::query(
            r#"
                INSERT INTO provers (batch_size, url, timeout_s, prover_type)
                VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(batch_size as i64)
        .bind(url)
        .bind(timeout_seconds as i64)
        .bind(prover_type);

        self.execute(query).await?;

        Ok(())
    }

    async fn insert_provers(self, provers: HashSet<ProverConfig>) -> Result<(), Error> {
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

        self.execute(query).await?;
        Ok(())
    }

    async fn remove_prover(self, batch_size: usize, prover_type: ProverType) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
              DELETE FROM provers WHERE batch_size = $1 AND prover_type = $2
            "#,
        )
        .bind(batch_size as i64)
        .bind(prover_type);

        self.execute(query).await?;

        Ok(())
    }

    async fn insert_new_identity(
        self,
        identity: Hash,
        eligibility_timestamp: sqlx::types::chrono::DateTime<Utc>,
    ) -> Result<Hash, Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO unprocessed_identities (commitment, status, created_at, eligibility)
            VALUES ($1, $2, CURRENT_TIMESTAMP, $3)
            "#,
        )
        .bind(identity)
        .bind(<&str>::from(UnprocessedStatus::New))
        .bind(eligibility_timestamp);

        self.execute(query).await?;
        Ok(identity)
    }

    async fn insert_new_recovery(
        self,
        existing_commitment: &Hash,
        new_commitment: &Hash,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO recoveries (existing_commitment, new_commitment)
            VALUES ($1, $2)
            "#,
        )
        .bind(existing_commitment)
        .bind(new_commitment);
        self.execute(query).await?;
        Ok(())
    }

    async fn get_latest_deletion(self) -> Result<LatestDeletionEntry, Error> {
        let query =
            sqlx::query("SELECT deletion_timestamp FROM latest_deletion_root WHERE Lock = 'X';");

        let row = self.fetch_optional(query).await?;

        if let Some(row) = row {
            Ok(LatestDeletionEntry {
                timestamp: row.get(0),
            })
        } else {
            Ok(LatestDeletionEntry {
                timestamp: Utc::now(),
            })
        }
    }

    async fn update_latest_insertion(
        self,
        insertion_timestamp: DateTime<Utc>,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO latest_insertion_timestamp (Lock, insertion_timestamp)
            VALUES ('X', $1)
            ON CONFLICT (Lock)
            DO UPDATE SET insertion_timestamp = EXCLUDED.insertion_timestamp;
            "#,
        )
        .bind(insertion_timestamp);

        self.execute(query).await?;
        Ok(())
    }

    async fn update_latest_deletion(self, deletion_timestamp: DateTime<Utc>) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO latest_deletion_root (Lock, deletion_timestamp)
            VALUES ('X', $1)
            ON CONFLICT (Lock)
            DO UPDATE SET deletion_timestamp = EXCLUDED.deletion_timestamp;
            "#,
        )
        .bind(deletion_timestamp);

        self.execute(query).await?;
        Ok(())
    }

    async fn get_all_recoveries(self) -> Result<Vec<RecoveryEntry>, Error> {
        Ok(
            sqlx::query_as::<_, RecoveryEntry>("SELECT * FROM recoveries")
                .fetch_all(self)
                .await?,
        )
    }

    async fn delete_recoveries<I: IntoIterator<Item = T>, T: Into<U256>>(
        self,
        prev_commits: I,
    ) -> Result<Vec<RecoveryEntry>, Error> {
        // TODO: upstream PgHasArrayType impl to ruint
        let prev_commits = prev_commits
            .into_iter()
            .map(|c| c.into().to_be_bytes())
            .collect::<Vec<[u8; 32]>>();

        let res = sqlx::query_as::<_, RecoveryEntry>(
            r#"
            DELETE
            FROM recoveries
            WHERE existing_commitment = ANY($1)
            RETURNING *
            "#,
        )
        .bind(&prev_commits)
        .fetch_all(self)
        .await?;

        Ok(res)
    }

    async fn insert_new_deletion(self, leaf_index: usize, identity: &Hash) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO deletions (leaf_index, commitment)
            VALUES ($1, $2)
            "#,
        )
        .bind(leaf_index as i64)
        .bind(identity);

        self.execute(query).await?;
        Ok(())
    }

    async fn count_deletions(self) -> Result<i32, Error> {
        let query = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM deletions
            "#,
        );
        let result = self.fetch_one(query).await?;
        Ok(result.get::<i64, _>(0) as i32)
    }

    // TODO: consider using a larger value than i64 for leaf index, ruint should
    // have postgres compatibility for u256
    async fn get_deletions(self) -> Result<Vec<DeletionEntry>, Error> {
        let query = sqlx::query(
            r#"
            SELECT *
            FROM deletions
            "#,
        );

        let result = self.fetch_all(query).await?;

        Ok(result
            .into_iter()
            .map(|row| DeletionEntry {
                leaf_index: row.get::<i64, _>(0) as usize,
                commitment: row.get::<Hash, _>(1),
            })
            .collect::<Vec<DeletionEntry>>())
    }

    /// Remove a list of entries from the deletions table
    async fn remove_deletions(self, commitments: &[Hash]) -> Result<(), Error> {
        let commitments = commitments
            .iter()
            .map(|c| c.to_be_bytes())
            .collect::<Vec<[u8; 32]>>();

        sqlx::query("DELETE FROM deletions WHERE commitment = Any($1)")
            .bind(commitments)
            .execute(self)
            .await?;

        Ok(())
    }

    async fn get_eligible_unprocessed_commitments(
        self,
        status: UnprocessedStatus,
    ) -> Result<Vec<types::UnprocessedCommitment>, Error> {
        let query = sqlx::query(
            r#"
                SELECT * FROM unprocessed_identities
                WHERE status = $1 AND CURRENT_TIMESTAMP > eligibility
                LIMIT $2
            "#,
        )
        .bind(<&str>::from(status))
        .bind(MAX_UNPROCESSED_FETCH_COUNT);

        let result = self.fetch_all(query).await?;

        Ok(result
            .into_iter()
            .map(|row| types::UnprocessedCommitment {
                commitment: row.get::<Hash, _>(0),
                status,
                created_at: row.get::<_, _>(2),
                processed_at: row.get::<_, _>(3),
                error_message: row.get::<_, _>(4),
                eligibility_timestamp: row.get::<_, _>(5),
            })
            .collect::<Vec<_>>())
    }

    async fn get_unprocessed_error(self, commitment: &Hash) -> Result<Option<String>, Error> {
        let query = sqlx::query(
            r#"
                SELECT error_message FROM unprocessed_identities WHERE commitment = $1
            "#,
        )
        .bind(commitment);

        let result = self.fetch_optional(query).await?;

        if let Some(row) = result {
            return Ok(Some(row.get::<Option<String>, _>(0).unwrap_or_default()));
        };
        Ok(None)
    }

    async fn remove_unprocessed_identity(self, commitment: &Hash) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
                DELETE FROM unprocessed_identities WHERE commitment = $1
            "#,
        )
        .bind(commitment);

        self.execute(query).await?;

        Ok(())
    }

    async fn identity_exists(self, commitment: Hash) -> Result<bool, Error> {
        Ok(sqlx::query(
            r#"
            select
            EXISTS (select commitment from unprocessed_identities where commitment = $1) OR
            EXISTS (select commitment from identities where commitment = $1);
            "#,
        )
        .bind(commitment)
        .fetch_one(self)
        .await?
        .get::<bool, _>(0))
    }

    // TODO: add docs
    async fn identity_is_queued_for_deletion(self, commitment: &Hash) -> Result<bool, Error> {
        let query_queued_deletion =
            sqlx::query(r#"SELECT exists(SELECT 1 FROM deletions where commitment = $1)"#)
                .bind(commitment);
        let row_unprocessed = self.fetch_one(query_queued_deletion).await?;
        Ok(row_unprocessed.get::<bool, _>(0))
    }

    async fn insert_new_batch_head(self, next_root: &Hash) -> Result<(), Error> {
        let query = sqlx::query(
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
            indexes:    vec![],
        }));

        self.execute(query).await?;
        Ok(())
    }

    async fn insert_new_batch(
        self,
        next_root: &Hash,
        prev_root: &Hash,
        batch_type: BatchType,
        identities: &[Identity],
        indexes: &[usize],
    ) -> Result<(), Error> {
        let query = sqlx::query(
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
            indexes:    indexes.to_vec(),
        }));

        self.execute(query).await?;
        Ok(())
    }

    async fn get_next_batch(self, prev_root: &Hash) -> Result<Option<BatchEntry>, Error> {
        let res = sqlx::query_as::<_, BatchEntry>(
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
        .fetch_optional(self)
        .await?;

        Ok(res)
    }

    async fn get_latest_batch(self) -> Result<Option<BatchEntry>, Error> {
        let res = sqlx::query_as::<_, BatchEntry>(
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
        .fetch_optional(self)
        .await?;

        Ok(res)
    }

    async fn get_latest_batch_with_transaction(self) -> Result<Option<BatchEntry>, Error> {
        let res = sqlx::query_as::<_, BatchEntry>(
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
            WHERE transactions.batch_next_root IS NOT NULL AND batches.prev_root IS NOT NULL
            ORDER BY batches.id DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(self)
        .await?;

        Ok(res)
    }

    async fn get_next_batch_without_transaction(self) -> Result<Option<BatchEntry>, Error> {
        let res = sqlx::query_as::<_, BatchEntry>(
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
        .fetch_optional(self)
        .await?;

        Ok(res)
    }

    async fn get_batch_head(self) -> Result<Option<BatchEntry>, Error> {
        let res = sqlx::query_as::<_, BatchEntry>(
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
        .fetch_optional(self)
        .await?;

        Ok(res)
    }

    async fn get_all_batches_after(self, id: i64) -> Result<Vec<BatchEntry>, Error> {
        let res = sqlx::query_as::<_, BatchEntry>(
            r#"
            SELECT
                id,
                next_root,
                prev_root,
                created_at,
                batch_type,
                data
            FROM batches WHERE id >= $1 ORDER BY id ASC
            "#,
        )
        .bind(id)
        .fetch_all(self)
        .await?;

        Ok(res)
    }

    #[instrument(skip(self), level = "debug")]
    async fn delete_batches_after_root(self, root: &Hash) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            DELETE FROM batches
            WHERE prev_root = $1
            "#,
        )
        .bind(root);

        self.execute(query).await?;
        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn delete_all_batches(self) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            DELETE FROM batches
            "#,
        );

        self.execute(query).await?;
        Ok(())
    }

    async fn root_in_batch_chain(self, root: &Hash) -> Result<bool, Error> {
        let query = sqlx::query(
            r#"SELECT exists(SELECT 1 FROM batches where prev_root = $1 OR next_root = $1)"#,
        )
        .bind(root);
        let row_unprocessed = self.fetch_one(query).await?;
        Ok(row_unprocessed.get::<bool, _>(0))
    }

    async fn insert_new_transaction(
        self,
        transaction_id: &String,
        batch_next_root: &Hash,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO transactions(
                transaction_id,
                batch_next_root,
                created_at
            ) VALUES ($1, $2, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(transaction_id)
        .bind(batch_next_root);

        self.execute(query).await?;
        Ok(())
    }

    async fn get_transaction_for_batch(
        self,
        next_root: &Hash,
    ) -> Result<Option<TransactionEntry>, Error> {
        let res = sqlx::query_as::<_, TransactionEntry>(
            r#"
            SELECT
                transaction_id,
                batch_next_root,
                created_at
            FROM transactions WHERE batch_next_root = $1
            LIMIT 1
            "#,
        )
        .bind(next_root)
        .fetch_optional(self)
        .await?;

        Ok(res)
    }
}
