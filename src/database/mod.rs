#![allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

use std::cmp::Ordering;
use std::collections::HashSet;
use std::ops::Deref;

use anyhow::{anyhow, Context, Error as ErrReport};
use chrono::{DateTime, Utc};
use ruint::aliases::U256;
use sqlx::migrate::{Migrate, MigrateDatabase, Migrator};
use sqlx::pool::PoolOptions;
use sqlx::{Executor, Pool, Postgres, Row};
use thiserror::Error;
use tracing::{error, info, instrument, warn};

use self::types::{CommitmentHistoryEntry, DeletionEntry, LatestDeletionEntry, RecoveryEntry};
use crate::config::DatabaseConfig;
use crate::database::types::{BatchEntry, BatchType, Commitments};
use crate::identity_tree::{
    Hash, ProcessedStatus, RootItem, TreeItem, TreeUpdate, UnprocessedStatus,
};
use crate::prover::{ProverConfig, ProverType};

pub mod types;

// Statically link in migration files
static MIGRATOR: Migrator = sqlx::migrate!("schemas/database");

const MAX_UNPROCESSED_FETCH_COUNT: i64 = 10_000;

pub struct Database {
    pub pool: Pool<Postgres>,
}

impl Deref for Database {
    type Target = Pool<Postgres>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

impl<'a, T> DatabaseExt<'a> for T where T: Executor<'a, Database = Postgres> {}

impl Database {
    #[instrument(skip_all)]
    pub async fn new(config: &DatabaseConfig) -> Result<Self, ErrReport> {
        info!(url = %&config.database, "Connecting to database");

        // Create database if requested and does not exist
        if config.migrate && !Postgres::database_exists(config.database.expose()).await? {
            warn!(url = %&config.database, "Database does not exist, creating database");
            Postgres::create_database(config.database.expose()).await?;
        }

        // Create a connection pool
        let pool = PoolOptions::<Postgres>::new()
            .max_connections(config.max_connections)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    conn.execute("SET DEFAULT_TRANSACTION_ISOLATION TO 'SERIALIZABLE'")
                        .await?;
                    Ok(())
                })
            })
            .connect(config.database.expose())
            .await
            .context("error connecting to database")?;

        let version = pool
            .fetch_one("SELECT version()")
            .await
            .context("error getting database version")?
            .get::<String, _>(0);
        info!(url = %&config.database, ?version, "Connected to database");

        // Run migrations if requested.
        let latest = MIGRATOR
            .migrations
            .last()
            .expect("Missing migrations")
            .version;

        if config.migrate {
            info!(url = %&config.database, "Running migrations");
            MIGRATOR.run(&pool).await?;
        }

        // Validate database schema version
        let mut conn = pool.acquire().await?;

        if conn.dirty_version().await?.is_some() {
            error!(
                url = %&config.database,
                version,
                expected = latest,
                "Database is in incomplete migration state.",
            );
            return Err(anyhow!("Database is in incomplete migration state."));
        }

        let version = conn
            .list_applied_migrations()
            .await?
            .last()
            .expect("Missing migrations")
            .version;

        match version.cmp(&latest) {
            Ordering::Less => {
                error!(
                    url = %&config.database,
                    version,
                    expected = latest,
                    "Database is not up to date, try rerunning with --database-migrate",         );
                return Err(anyhow!(
                    "Database is not up to date, try rerunning with --database-migrate"
                ));
            }
            Ordering::Greater => {
                error!(
                    url = %&config.database,
                    version,
                    latest,
                    "Database version is newer than this version of the software, please update.",         );
                return Err(anyhow!(
                    "Database version is newer than this version of the software, please update."
                ));
            }
            Ordering::Equal => {
                info!(
                    url = %&config.database,
                    version,
                    latest,
                    "Database version is up to date.",
                );
            }
        }

        Ok(Self { pool })
    }

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

/// This trait provides the individual and composable queries to the database.
/// Each method is a single atomic query, and can be composed withing a
/// transaction.
pub trait DatabaseExt<'a>: Executor<'a, Database = Postgres> {
    async fn insert_pending_identity(
        self,
        leaf_index: usize,
        identity: &Hash,
        root: &Hash,
    ) -> Result<(), Error> {
        let insert_pending_identity_query = sqlx::query(
            r#"
            INSERT INTO identities (leaf_index, commitment, root, status, pending_as_of)
            VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(leaf_index as i64)
        .bind(identity)
        .bind(root)
        .bind(<&str>::from(ProcessedStatus::Pending));

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
            SELECT leaf_index, commitment as element
            FROM identities
            WHERE status = $1
            ORDER BY id ASC;
            "#,
        )
        .bind(<&str>::from(status))
        .fetch_all(self)
        .await?)
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

    async fn get_latest_insertion_timestamp(self) -> Result<Option<DateTime<Utc>>, Error> {
        let query = sqlx::query(
            r#"
            SELECT insertion_timestamp
            FROM latest_insertion_timestamp
            WHERE Lock = 'X';"#,
        );

        let row = self.fetch_optional(query).await?;

        Ok(row.map(|r| r.get::<DateTime<Utc>, _>(0)))
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

    async fn update_latest_insertion_timestamp(
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

    async fn get_unprocessed_commit_status(
        self,
        commitment: &Hash,
    ) -> Result<Option<(UnprocessedStatus, String)>, Error> {
        let query = sqlx::query(
            r#"
                SELECT status, error_message FROM unprocessed_identities WHERE commitment = $1
            "#,
        )
        .bind(commitment);

        let result = self.fetch_optional(query).await?;

        if let Some(row) = result {
            return Ok(Some((
                row.get::<&str, _>(0).parse().expect("couldn't read status"),
                row.get::<Option<String>, _>(1).unwrap_or_default(),
            )));
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

    async fn insert_new_batch_head(
        self,
        next_root: &Hash,
        batch_type: BatchType,
        commitments: &Commitments,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO batches(
                next_root,
                prev_root,
                created_at,
                batch_type,
                commitments
            ) VALUES ($1, NULL, CURRENT_TIMESTAMP, $2, $3)
            "#,
        )
        .bind(next_root)
        .bind(batch_type)
        .bind(commitments);

        self.execute(query).await?;
        Ok(())
    }

    async fn insert_new_batch(
        self,
        next_root: &Hash,
        prev_root: &Hash,
        batch_type: BatchType,
        commitments: &Commitments,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO batches(
                next_root,
                prev_root,
                created_at,
                batch_type,
                commitments
            ) VALUES ($1, $2, CURRENT_TIMESTAMP, $3, $4)
            "#,
        )
        .bind(next_root)
        .bind(prev_root)
        .bind(batch_type)
        .bind(commitments);

        self.execute(query).await?;
        Ok(())
    }

    async fn get_next_batch(self, prev_root: &Hash) -> Result<Option<BatchEntry>, Error> {
        let res = sqlx::query_as::<_, BatchEntry>(
            r#"
            SELECT * FROM batches WHERE prev_root = $1
            "#,
        )
        .bind(prev_root)
        .fetch_optional(self)
        .await?;

        Ok(res)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    InternalError(#[from] sqlx::Error),

    #[error("Tried to mine missing root {root:?}")]
    MissingRoot { root: Hash },
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;
    use std::str::FromStr;
    use std::time::Duration;

    use anyhow::Context;
    use chrono::{Days, Utc};
    use ethers::types::U256;
    use postgres_docker_utils::DockerContainer;
    use ruint::Uint;
    use semaphore::Field;
    use testcontainers::clients::Cli;

    use super::Database;
    use crate::config::DatabaseConfig;
    use crate::database::types::{BatchType, Commitments};
    use crate::database::DatabaseExt as _;
    use crate::identity_tree::{Hash, ProcessedStatus, Status, UnprocessedStatus};
    use crate::prover::{ProverConfig, ProverType};
    use crate::utils::secret::SecretUrl;

    macro_rules! assert_same_time {
        ($a:expr, $b:expr, $diff:expr) => {
            assert!(
                abs_duration($a - $b) < $diff,
                "Difference between {} and {} is larger than {:?}",
                $a,
                $b,
                $diff
            );
        };

        ($a:expr, $b:expr) => {
            assert_same_time!($a, $b, chrono::Duration::milliseconds(500));
        };
    }

    fn abs_duration(x: chrono::Duration) -> chrono::Duration {
        chrono::Duration::milliseconds(x.num_milliseconds().abs())
    }

    // TODO: we should probably consolidate all tests that propagate errors to
    // TODO: either use anyhow or eyre
    async fn setup_db<'a>(docker: &'a Cli) -> anyhow::Result<(Database, DockerContainer)> {
        let db_container = postgres_docker_utils::setup(docker).await?;
        let url = format!(
            "postgres://postgres:postgres@{}/database",
            db_container.address()
        );

        let db = Database::new(&DatabaseConfig {
            database:        SecretUrl::from_str(&url)?,
            migrate:         true,
            max_connections: 1,
        })
        .await?;

        Ok((db, db_container))
    }

    fn mock_roots(n: usize) -> Vec<Field> {
        (1..=n).map(Field::from).collect()
    }

    fn mock_zero_roots(n: usize) -> Vec<Field> {
        const ZERO_ROOT_OFFSET: usize = 10_000_000;

        (1..=n)
            .map(|n| ZERO_ROOT_OFFSET - n)
            .map(Field::from)
            .collect()
    }

    fn mock_identities(n: usize) -> Vec<Field> {
        (1..=n).map(Field::from).collect()
    }

    async fn assert_roots_are(
        db: &Database,
        roots: impl IntoIterator<Item = &Field>,
        expected_state: ProcessedStatus,
    ) -> anyhow::Result<()> {
        for root in roots {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, expected_state,);
        }

        Ok(())
    }

    #[tokio::test]
    async fn insert_identity() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let dec = "1234500000000000000";
        let commit_hash: Hash = U256::from_dec_str(dec)
            .expect("cant convert to u256")
            .into();

        let eligibility_timestamp = Utc::now();

        let hash = db
            .insert_new_identity(commit_hash, eligibility_timestamp)
            .await?;

        assert_eq!(commit_hash, hash);

        let commit = db
            .get_unprocessed_commit_status(&commit_hash)
            .await?
            .expect("expected commitment status");
        assert_eq!(commit.0, UnprocessedStatus::New);

        let identity_count = db
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?
            .len();

        assert_eq!(identity_count, 1);

        assert!(db.remove_unprocessed_identity(&commit_hash).await.is_ok());

        Ok(())
    }

    #[tokio::test]
    async fn insert_and_delete_identity() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let zero: Hash = U256::zero().into();
        let zero_root: Hash = U256::from_dec_str("6789")?.into();
        let root: Hash = U256::from_dec_str("54321")?.into();
        let commitment: Hash = U256::from_dec_str("12345")?.into();

        db.insert_pending_identity(0, &commitment, &root).await?;
        db.insert_pending_identity(0, &zero, &zero_root).await?;

        let leaf_index = db
            .get_identity_leaf_index(&commitment)
            .await?
            .context("Missing identity")?;

        assert_eq!(leaf_index.leaf_index, 0);

        Ok(())
    }

    fn mock_provers() -> HashSet<ProverConfig> {
        let mut provers = HashSet::new();

        provers.insert(ProverConfig {
            batch_size:  100,
            url:         "http://localhost:8080".to_string(),
            timeout_s:   100,
            prover_type: ProverType::Insertion,
        });

        provers.insert(ProverConfig {
            batch_size:  100,
            url:         "http://localhost:8080".to_string(),
            timeout_s:   100,
            prover_type: ProverType::Deletion,
        });

        provers
    }

    #[tokio::test]
    async fn test_insert_prover_configuration() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let mock_prover_configuration_0 = ProverConfig {
            batch_size:  100,
            url:         "http://localhost:8080".to_string(),
            timeout_s:   100,
            prover_type: ProverType::Insertion,
        };

        let mock_prover_configuration_1 = ProverConfig {
            batch_size:  100,
            url:         "http://localhost:8081".to_string(),
            timeout_s:   100,
            prover_type: ProverType::Deletion,
        };

        db.insert_prover_configuration(
            mock_prover_configuration_0.batch_size,
            mock_prover_configuration_0.url.clone(),
            mock_prover_configuration_0.timeout_s,
            mock_prover_configuration_0.prover_type,
        )
        .await?;

        db.insert_prover_configuration(
            mock_prover_configuration_1.batch_size,
            mock_prover_configuration_1.url.clone(),
            mock_prover_configuration_1.timeout_s,
            mock_prover_configuration_1.prover_type,
        )
        .await?;

        let provers = db.get_provers().await?;

        assert!(provers.contains(&mock_prover_configuration_0));
        assert!(provers.contains(&mock_prover_configuration_1));

        Ok(())
    }

    #[tokio::test]
    async fn test_insert_provers() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let mock_provers = mock_provers();

        db.insert_provers(mock_provers.clone()).await?;

        let provers = db.get_provers().await?;

        assert_eq!(provers, mock_provers);
        Ok(())
    }

    #[tokio::test]
    async fn test_remove_prover() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let mock_provers = mock_provers();

        db.insert_provers(mock_provers.clone()).await?;

        db.remove_prover(100, ProverType::Insertion).await?;
        db.remove_prover(100, ProverType::Deletion).await?;

        let provers = db.get_provers().await?;

        assert_eq!(provers, HashSet::new());

        Ok(())
    }

    #[tokio::test]
    async fn test_insert_new_recovery() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let existing_commitment: Uint<256, 4> = Uint::from(1);
        let new_commitment: Uint<256, 4> = Uint::from(2);

        db.insert_new_recovery(&existing_commitment, &new_commitment)
            .await?;

        let recoveries = db.get_all_recoveries().await?;

        assert_eq!(recoveries.len(), 1);
        assert_eq!(recoveries[0].existing_commitment, existing_commitment);
        assert_eq!(recoveries[0].new_commitment, new_commitment);

        Ok(())
    }

    #[tokio::test]
    async fn test_insert_new_deletion() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let existing_commitment: Uint<256, 4> = Uint::from(1);

        db.insert_new_deletion(0, &existing_commitment).await?;

        let deletions = db.get_deletions().await?;
        assert_eq!(deletions.len(), 1);
        assert_eq!(deletions[0].leaf_index, 0);
        assert_eq!(deletions[0].commitment, existing_commitment);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_eligible_unprocessed_commitments() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let commitment_0: Uint<256, 4> = Uint::from(1);
        let eligibility_timestamp_0 = Utc::now();

        db.insert_new_identity(commitment_0, eligibility_timestamp_0)
            .await?;

        let commitment_1: Uint<256, 4> = Uint::from(2);
        let eligibility_timestamp_1 = Utc::now()
            .checked_add_days(Days::new(7))
            .expect("Could not create eligibility timestamp");

        db.insert_new_identity(commitment_1, eligibility_timestamp_1)
            .await?;

        let unprocessed_commitments = db
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?;

        assert_eq!(unprocessed_commitments.len(), 1);
        assert_eq!(unprocessed_commitments[0].commitment, commitment_0);
        assert!(
            unprocessed_commitments[0].eligibility_timestamp.timestamp()
                - eligibility_timestamp_0.timestamp()
                <= 1
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_get_unprocessed_commitments() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        // Insert new identity with a valid eligibility timestamp
        let commitment_0: Uint<256, 4> = Uint::from(1);
        let eligibility_timestamp_0 = Utc::now();
        db.insert_new_identity(commitment_0, eligibility_timestamp_0)
            .await?;

        // Insert new identity with eligibility timestamp in the future
        let commitment_1: Uint<256, 4> = Uint::from(2);
        let eligibility_timestamp_1 = Utc::now()
            .checked_add_days(Days::new(7))
            .expect("Could not create eligibility timestamp");
        db.insert_new_identity(commitment_1, eligibility_timestamp_1)
            .await?;

        let unprocessed_commitments = db
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?;

        // Assert unprocessed commitments against expected values
        assert_eq!(unprocessed_commitments.len(), 1);
        assert_eq!(unprocessed_commitments[0].commitment, commitment_0);
        assert_eq!(
            unprocessed_commitments[0].eligibility_timestamp.timestamp(),
            eligibility_timestamp_0.timestamp()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_identity_is_queued_for_deletion() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let existing_commitment: Uint<256, 4> = Uint::from(1);

        db.insert_new_deletion(0, &existing_commitment).await?;

        assert!(
            db.identity_is_queued_for_deletion(&existing_commitment)
                .await?
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_update_eligibility_timestamp() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let dec = "1234500000000000000";
        let commit_hash: Hash = U256::from_dec_str(dec)
            .expect("cant convert to u256")
            .into();

        // Set eligibility to Utc::now() day and check db entries
        let eligibility_timestamp = Utc::now();
        db.insert_new_identity(commit_hash, eligibility_timestamp)
            .await?;

        let commitments = db
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?;
        assert_eq!(commitments.len(), 1);

        let eligible_commitments = db
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?;
        assert_eq!(eligible_commitments.len(), 1);

        // Set eligibility to Utc::now() + 7 days and check db entries
        let eligibility_timestamp = Utc::now()
            .checked_add_days(Days::new(7))
            .expect("Could not create eligibility timestamp");

        // Insert new identity with an eligibility timestamp in the future
        let commit_hash: Hash = Hash::from(1);
        db.insert_new_identity(commit_hash, eligibility_timestamp)
            .await?;

        let eligible_commitments = db
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?;
        assert_eq!(eligible_commitments.len(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_update_insertion_timestamp() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let insertion_timestamp = Utc::now();

        db.update_latest_insertion_timestamp(insertion_timestamp)
            .await?;

        let latest_insertion_timestamp = db.get_latest_insertion_timestamp().await?.unwrap();

        assert!(latest_insertion_timestamp.timestamp() - insertion_timestamp.timestamp() <= 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_insert_deletion() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities = mock_identities(3);

        db.insert_new_deletion(0, &identities[0]).await?;
        db.insert_new_deletion(1, &identities[1]).await?;
        db.insert_new_deletion(2, &identities[2]).await?;

        let deletions = db.get_deletions().await?;

        assert_eq!(deletions.len(), 3);

        Ok(())
    }

    #[tokio::test]
    async fn test_insert_recovery() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let old_identities = mock_identities(3);
        let new_identities = mock_identities(3);

        for (old, new) in old_identities.into_iter().zip(new_identities) {
            db.insert_new_recovery(&old, &new).await?;
        }

        let recoveries = db.get_all_recoveries().await?;
        assert_eq!(recoveries.len(), 3);

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_recoveries() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let old_identities = mock_identities(3);
        let new_identities = mock_identities(3);

        for (old, new) in old_identities.clone().into_iter().zip(new_identities) {
            db.insert_new_recovery(&old, &new).await?;
        }

        let deleted_recoveries = db
            .delete_recoveries(old_identities[0..2].iter().cloned())
            .await?;
        assert_eq!(deleted_recoveries.len(), 2);

        let remaining = db.get_all_recoveries().await?;
        assert_eq!(remaining.len(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn get_last_leaf_index() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(1);
        let roots = mock_roots(1);

        let next_leaf_index = db.get_next_leaf_index().await?;

        assert_eq!(next_leaf_index, 0, "Db should contain not leaf indexes");

        db.insert_pending_identity(0, &identities[0], &roots[0])
            .await?;

        let next_leaf_index = db.get_next_leaf_index().await?;
        assert_eq!(next_leaf_index, 1);

        Ok(())
    }

    #[tokio::test]
    async fn mark_all_as_pending_marks_all() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(5);
        let roots = mock_roots(5);

        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], &roots[i])
                .await
                .context("Inserting identity")?;
        }

        db.mark_root_as_processed(&roots[2]).await?;

        db.mark_all_as_pending().await?;

        for root in &roots {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, ProcessedStatus::Pending);
        }

        Ok(())
    }

    #[tokio::test]
    async fn mark_root_as_processed_marks_previous_roots() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(5);
        let roots = mock_roots(5);

        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], &roots[i])
                .await
                .context("Inserting identity")?;
        }

        db.mark_root_as_processed(&roots[2]).await?;

        for root in roots.iter().take(3) {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, ProcessedStatus::Processed);
        }

        for root in roots.iter().skip(3).take(2) {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, ProcessedStatus::Pending);
        }

        let pending_identities = db.count_pending_identities().await?;
        assert_eq!(
            pending_identities, 2,
            "There should be 2 pending identities"
        );

        Ok(())
    }

    #[tokio::test]
    async fn mark_root_as_mined_marks_previous_roots() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(5);
        let roots = mock_roots(5);

        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], &roots[i])
                .await
                .context("Inserting identity")?;
        }

        db.mark_root_as_mined(&roots[2]).await?;

        for root in roots.iter().take(3) {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, ProcessedStatus::Mined);
        }

        for root in roots.iter().skip(3).take(2) {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, ProcessedStatus::Pending);
        }

        let pending_identities = db.count_pending_identities().await?;
        assert_eq!(
            pending_identities, 2,
            "There should be 2 pending identities"
        );

        Ok(())
    }

    #[tokio::test]
    async fn mark_root_as_mined_interaction_with_mark_root_as_processed() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let num_identities = 6;

        let identities = mock_identities(num_identities);
        let roots = mock_roots(num_identities);

        for i in 0..num_identities {
            db.insert_pending_identity(i, &identities[i], &roots[i])
                .await
                .context("Inserting identity")?;
        }

        println!("Marking roots up to 2nd as processed");
        db.mark_root_as_processed(&roots[2]).await?;

        assert_roots_are(&db, &roots[..3], ProcessedStatus::Processed).await?;
        assert_roots_are(&db, &roots[3..], ProcessedStatus::Pending).await?;

        println!("Marking roots up to 1st as mined");
        db.mark_root_as_mined(&roots[1]).await?;

        assert_roots_are(&db, &roots[..2], ProcessedStatus::Mined).await?;
        assert_roots_are(&db, &[roots[2]], ProcessedStatus::Processed).await?;
        assert_roots_are(&db, &roots[3..], ProcessedStatus::Pending).await?;

        println!("Marking roots up to 4th as processed");
        db.mark_root_as_processed(&roots[4]).await?;

        assert_roots_are(&db, &roots[..2], ProcessedStatus::Mined).await?;
        assert_roots_are(&db, &roots[2..5], ProcessedStatus::Processed).await?;
        assert_roots_are(&db, &roots[5..], ProcessedStatus::Pending).await?;

        println!("Marking all roots as mined");
        db.mark_root_as_mined(&roots[num_identities - 1]).await?;

        assert_roots_are(&db, &roots, ProcessedStatus::Mined).await?;

        Ok(())
    }

    #[tokio::test]
    async fn mark_root_as_processed_marks_next_roots() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(5);
        let roots = mock_roots(5);

        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], &roots[i])
                .await
                .context("Inserting identity")?;
        }

        // root[2] is somehow erroneously marked as mined
        db.mark_root_as_processed(&roots[2]).await?;

        // Later we correctly mark the previous root as mined
        db.mark_root_as_processed(&roots[1]).await?;

        for root in roots.iter().take(2) {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, ProcessedStatus::Processed);
        }

        for root in roots.iter().skip(2).take(3) {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, ProcessedStatus::Pending);
        }

        let pending_identities = db.count_pending_identities().await?;
        assert_eq!(pending_identities, 3, "There should be 3 pending roots");

        Ok(())
    }

    #[tokio::test]
    async fn root_history_timing() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(5);
        let roots = mock_roots(5);

        let root = db.get_root_state(&roots[0]).await?;

        assert!(root.is_none(), "Root should not exist");

        db.insert_pending_identity(0, &identities[0], &roots[0])
            .await?;

        let root = db
            .get_root_state(&roots[0])
            .await?
            .context("Fetching root state")?;
        let time_of_insertion = Utc::now();

        assert_same_time!(
            root.pending_valid_as_of,
            time_of_insertion,
            chrono::Duration::milliseconds(100)
        );

        assert!(
            root.mined_valid_as_of.is_none(),
            "Root has not yet been mined"
        );

        db.mark_root_as_processed(&roots[0]).await?;

        let root = db
            .get_root_state(&roots[0])
            .await?
            .context("Fetching root state")?;
        let time_of_mining = Utc::now();

        let mined_valid_as_of = root
            .mined_valid_as_of
            .context("Root should have been mined")?;

        assert_same_time!(
            root.pending_valid_as_of,
            time_of_insertion,
            chrono::Duration::milliseconds(100)
        );
        assert_same_time!(
            mined_valid_as_of,
            time_of_mining,
            chrono::Duration::milliseconds(100)
        );

        Ok(())
    }

    #[tokio::test]
    async fn get_commitments_by_status() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(5);

        let roots = mock_roots(7);

        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], &roots[i])
                .await
                .context("Inserting identity")?;
        }

        db.mark_root_as_processed(&roots[2]).await?;

        let mined_tree_updates = db
            .get_commitments_by_status(ProcessedStatus::Processed)
            .await?;
        let pending_tree_updates = db
            .get_commitments_by_status(ProcessedStatus::Pending)
            .await?;

        assert_eq!(mined_tree_updates.len(), 3);
        for i in 0..3 {
            assert_eq!(mined_tree_updates[i].element, identities[i]);
            assert_eq!(mined_tree_updates[i].leaf_index, i);
        }

        assert_eq!(pending_tree_updates.len(), 2);
        for i in 0..2 {
            assert_eq!(pending_tree_updates[i].element, identities[i + 3]);
            assert_eq!(pending_tree_updates[i].leaf_index, i + 3);
        }

        Ok(())
    }

    #[tokio::test]
    async fn get_commitments_by_status_results_are_in_id_order() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(5);

        let roots = mock_roots(5);
        let zero_roots = mock_zero_roots(5);

        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], &roots[i])
                .await
                .context("Inserting identity")?;
        }

        db.insert_pending_identity(0, &Hash::ZERO, &zero_roots[0])
            .await?;
        db.insert_pending_identity(3, &Hash::ZERO, &zero_roots[3])
            .await?;

        let pending_tree_updates = db
            .get_commitments_by_status(ProcessedStatus::Pending)
            .await?;
        assert_eq!(pending_tree_updates.len(), 7);

        // 1st identity
        assert_eq!(
            pending_tree_updates[0].element, identities[0],
            "First element is the original value"
        );

        for (idx, identity) in identities.iter().enumerate() {
            assert_eq!(
                pending_tree_updates[idx].element, *identity,
                "Element {} is the original value",
                idx
            );
        }

        // Deletions
        assert_eq!(
            pending_tree_updates[5].element,
            Hash::ZERO,
            "First deletion is at the end"
        );
        assert_eq!(
            pending_tree_updates[6].element,
            Hash::ZERO,
            "Second deletion is at the end"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_root_invalidation() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(5);
        let roots = mock_roots(5);

        db.insert_pending_identity(0, &identities[0], &roots[0])
            .await
            .context("Inserting identity 1")?;

        tokio::time::sleep(Duration::from_secs(2)).await; // sleep enough for the database time resolution

        // Invalid root returns None
        assert!(db.get_root_state(&roots[1]).await?.is_none());

        // Basic scenario, latest pending root
        let root_item = db.get_root_state(&roots[0]).await?.unwrap();
        assert_eq!(roots[0], root_item.root);
        assert!(matches!(root_item.status, ProcessedStatus::Pending));
        assert!(root_item.mined_valid_as_of.is_none());

        // Inserting a new pending root sets invalidation time for the
        // previous root
        db.insert_pending_identity(1, &identities[1], &roots[1])
            .await?;
        db.insert_pending_identity(2, &identities[2], &roots[2])
            .await?;

        let root_1_inserted_at = Utc::now();

        tokio::time::sleep(Duration::from_secs(2)).await; // sleep enough for the database time resolution

        let root_item_0 = db.get_root_state(&roots[0]).await?.unwrap();
        let root_item_1 = db.get_root_state(&roots[1]).await?.unwrap();

        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);
        println!("root_1_inserted_at = {root_1_inserted_at:?}");
        println!(
            "root_item_1.pending_valid_as_of = {:?}",
            root_item_1.pending_valid_as_of
        );

        assert_same_time!(root_item_1.pending_valid_as_of, root_1_inserted_at);

        // Test mined roots
        db.insert_pending_identity(3, &identities[3], &roots[3])
            .await?;

        db.mark_root_as_processed(&roots[0])
            .await
            .context("Marking root as mined")?;

        let root_2_mined_at = Utc::now();

        tokio::time::sleep(Duration::from_secs(2)).await; // sleep enough for the database time resolution

        let root_item_2 = db.get_root_state(&roots[2]).await?.unwrap();
        assert!(matches!(root_item_2.status, ProcessedStatus::Pending));
        assert!(root_item_2.mined_valid_as_of.is_none());

        let root_item_1 = db.get_root_state(&roots[1]).await?.unwrap();
        assert_eq!(root_item_1.status, ProcessedStatus::Pending);
        assert!(root_item_1.mined_valid_as_of.is_none());
        assert!(root_item_1.pending_valid_as_of < root_2_mined_at);

        let root_item_0 = db.get_root_state(&roots[0]).await?.unwrap();
        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);
        assert_eq!(root_item_0.status, ProcessedStatus::Processed);
        assert!(root_item_0.mined_valid_as_of.unwrap() < root_2_mined_at);
        assert!(root_item_0.mined_valid_as_of.unwrap() > root_1_inserted_at);
        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);

        Ok(())
    }

    #[tokio::test]
    async fn check_identity_existence() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(2);
        let roots = mock_roots(1);

        // When there's no identity
        assert!(!db.identity_exists(identities[0]).await?);

        // When there's only unprocessed identity
        let eligibility_timestamp = Utc::now();

        db.insert_new_identity(identities[0], eligibility_timestamp)
            .await
            .context("Inserting new identity")?;
        assert!(db.identity_exists(identities[0]).await?);

        // When there's only processed identity
        db.insert_pending_identity(0, &identities[1], &roots[0])
            .await
            .context("Inserting identity")?;

        assert!(db.identity_exists(identities[1]).await?);

        Ok(())
    }

    #[tokio::test]
    async fn test_remove_deletions() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(4);

        // Insert new identities
        db.insert_new_deletion(0, &identities[0])
            .await
            .context("Inserting new identity")?;

        db.insert_new_deletion(1, &identities[1])
            .await
            .context("Inserting new identity")?;

        db.insert_new_deletion(2, &identities[2])
            .await
            .context("Inserting new identity")?;
        db.insert_new_deletion(3, &identities[3])
            .await
            .context("Inserting new identity")?;

        // Remove identities 0 to 2
        db.remove_deletions(&identities[0..=2]).await?;
        let deletions = db.get_deletions().await?;

        assert_eq!(deletions.len(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_latest_deletion_root() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        // Update with initial timestamp
        let initial_timestamp = chrono::Utc::now();
        db.update_latest_deletion(initial_timestamp)
            .await
            .context("Inserting initial root")?;

        // Assert values
        let initial_entry = db.get_latest_deletion().await?;
        assert!(initial_entry.timestamp.timestamp() - initial_timestamp.timestamp() <= 1);

        // Update with a new timestamp
        let new_timestamp = chrono::Utc::now();
        db.update_latest_deletion(new_timestamp)
            .await
            .context("Updating with new root")?;

        // Assert values
        let new_entry = db.get_latest_deletion().await?;
        assert!((new_entry.timestamp.timestamp() - new_timestamp.timestamp()) <= 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_history_unprocessed_identities() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities = mock_identities(2);

        let now = Utc::now();

        let insertion_timestamp = now - chrono::Duration::seconds(5);
        db.insert_new_identity(identities[0], insertion_timestamp)
            .await?;

        let insertion_timestamp = now + chrono::Duration::seconds(5);
        db.insert_new_identity(identities[1], insertion_timestamp)
            .await?;

        let history = db.get_identity_history_entries(&identities[0]).await?;

        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].status,
            Status::Unprocessed(UnprocessedStatus::New)
        );
        assert!(!history[0].held_back, "Identity should not be held back");
        assert_eq!(history[0].leaf_index, None);

        let history = db.get_identity_history_entries(&identities[1]).await?;

        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].status,
            Status::Unprocessed(UnprocessedStatus::New)
        );
        assert!(history[0].held_back, "Identity should be held back");
        assert_eq!(history[0].leaf_index, None);

        Ok(())
    }

    #[tokio::test]
    async fn test_history_unprocessed_deletion_identities() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities = mock_identities(2);
        let roots = mock_roots(2);

        db.insert_pending_identity(0, &identities[0], &roots[0])
            .await?;
        db.mark_root_as_mined(&roots[0]).await?;

        db.insert_new_deletion(0, &identities[0]).await?;

        let history = db.get_identity_history_entries(&identities[0]).await?;

        assert_eq!(history.len(), 2);

        assert_eq!(history[0].status, Status::Processed(ProcessedStatus::Mined));
        assert_eq!(history[0].commitment, identities[0]);
        assert_eq!(history[0].leaf_index, Some(0));
        assert!(!history[0].held_back, "Identity should not be held back");

        assert_eq!(
            history[1].status,
            Status::Unprocessed(UnprocessedStatus::New)
        );
        assert_eq!(history[1].commitment, Hash::ZERO);
        assert_eq!(history[1].leaf_index, Some(0));
        assert!(!history[1].held_back, "Identity should not be held back");

        Ok(())
    }

    #[tokio::test]
    async fn test_history_processed_deletion_identities() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities = mock_identities(2);
        let roots = mock_roots(2);

        db.insert_pending_identity(0, &identities[0], &roots[0])
            .await?;
        db.insert_pending_identity(0, &Hash::ZERO, &roots[1])
            .await?;

        db.mark_root_as_mined(&roots[1]).await?;

        let history = db.get_identity_history_entries(&identities[0]).await?;

        assert_eq!(history.len(), 2);

        assert_eq!(history[0].status, Status::Processed(ProcessedStatus::Mined));
        assert_eq!(history[0].commitment, identities[0]);
        assert_eq!(history[0].leaf_index, Some(0));
        assert!(!history[0].held_back, "Identity should not be held back");

        assert_eq!(history[1].status, Status::Processed(ProcessedStatus::Mined));
        assert_eq!(history[1].commitment, Hash::ZERO);
        assert_eq!(history[1].leaf_index, Some(0));
        assert!(!history[1].held_back, "Identity should not be held back");

        Ok(())
    }

    #[tokio::test]
    async fn test_history_processed_identity() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities = mock_identities(2);
        let roots = mock_roots(2);

        db.insert_pending_identity(0, &identities[0], &roots[0])
            .await?;

        let history = db.get_identity_history_entries(&identities[0]).await?;

        assert_eq!(history.len(), 1);

        assert_eq!(
            history[0].status,
            Status::Processed(ProcessedStatus::Pending)
        );
        assert_eq!(history[0].commitment, identities[0]);
        assert_eq!(history[0].leaf_index, Some(0));
        assert!(!history[0].held_back, "Identity should not be held back");

        db.mark_root_as_mined(&roots[0]).await?;

        let history = db.get_identity_history_entries(&identities[0]).await?;

        assert_eq!(history.len(), 1);

        assert_eq!(history[0].status, Status::Processed(ProcessedStatus::Mined));
        assert_eq!(history[0].commitment, identities[0]);
        assert_eq!(history[0].leaf_index, Some(0));
        assert!(!history[0].held_back, "Identity should not be held back");

        Ok(())
    }

    #[tokio::test]
    async fn can_insert_same_root_multiple_times() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities = mock_identities(2);
        let roots = mock_roots(2);

        db.insert_pending_identity(0, &identities[0], &roots[0])
            .await?;

        db.insert_pending_identity(1, &identities[1], &roots[0])
            .await?;

        let root_state = db
            .get_root_state(&roots[0])
            .await?
            .context("Missing root")?;

        assert_eq!(root_state.status, ProcessedStatus::Pending);

        Ok(())
    }

    #[tokio::test]
    async fn test_insert_batch() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities = mock_identities(10);
        let identities1: Vec<Field> = identities
            .iter()
            .skip(0)
            .take(4)
            .map(|v| v.clone())
            .collect();
        let identities2: Vec<Field> = identities
            .iter()
            .skip(4)
            .take(6)
            .map(|v| v.clone())
            .collect();
        let roots = mock_roots(2);

        db.insert_new_batch_head(&roots[0], BatchType::Insertion, &Commitments(identities1))
            .await?;
        db.insert_new_batch(
            &roots[1],
            &roots[0],
            BatchType::Insertion,
            &Commitments(identities2),
        )
        .await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_get_next_batch() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities = mock_identities(10);
        let identities1: Vec<Field> = identities
            .iter()
            .skip(0)
            .take(4)
            .map(|v| v.clone())
            .collect();
        let identities2: Vec<Field> = identities
            .iter()
            .skip(4)
            .take(6)
            .map(|v| v.clone())
            .collect();
        let roots = mock_roots(2);

        db.insert_new_batch_head(&roots[0], BatchType::Insertion, &Commitments(identities1))
            .await?;
        db.insert_new_batch(
            &roots[1],
            &roots[0],
            BatchType::Insertion,
            &Commitments(identities2.clone()),
        )
        .await?;

        let next_batch = db.get_next_batch(&roots[0]).await?;

        assert!(next_batch.is_some());

        let next_batch = next_batch.unwrap();

        assert_eq!(next_batch.prev_root.unwrap(), roots[0]);
        assert_eq!(next_batch.next_root, roots[1]);
        assert_eq!(next_batch.commitments, identities2.into());

        let next_batch = db.get_next_batch(&roots[1]).await?;

        assert!(next_batch.is_none());

        Ok(())
    }
}
