#![allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

use std::collections::HashSet;

use anyhow::{anyhow, Context, Error as ErrReport};
use chrono::{DateTime, Utc};
use clap::Parser;
use sqlx::migrate::{Migrate, MigrateDatabase, Migrator};
use sqlx::pool::PoolOptions;
use sqlx::{Executor, Pool, Postgres, Row};
use thiserror::Error;
use tracing::{error, info, instrument, warn};

use self::types::{DeletionEntry, LatestDeletionEntry, RecoveryEntry};
use crate::identity_tree::{Hash, PendingStatus, RootItem, Status, TreeItem, TreeUpdate};

pub mod types;
use crate::prover::{ProverConfiguration, ProverType, Provers};
use crate::secret::SecretUrl;

// Statically link in migration files
static MIGRATOR: Migrator = sqlx::migrate!("schemas/database");

const MAX_UNPROCESSED_FETCH_COUNT: i64 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, Parser)]
pub struct Options {
    /// Database server connection string.
    /// Example: `postgres://user:password@localhost:5432/database`
    #[clap(long, env)]
    pub database: SecretUrl,

    /// Allow creation or migration of the database schema.
    #[clap(long, default_value = "true")]
    pub database_migrate: bool,

    /// Maximum number of connections in the database connection pool
    #[clap(long, env, default_value = "10")]
    pub database_max_connections: u32,
}

pub struct Database {
    pool: Pool<Postgres>,
}

impl Database {
    #[instrument(skip_all)]
    pub async fn new(options: Options) -> Result<Self, ErrReport> {
        info!(url = %&options.database, "Connecting to database");

        // Create database if requested and does not exist
        if options.database_migrate && !Postgres::database_exists(options.database.expose()).await?
        {
            warn!(url = %&options.database, "Database does not exist, creating database");
            Postgres::create_database(options.database.expose()).await?;
        }

        // Create a connection pool
        let pool = PoolOptions::<Postgres>::new()
            .max_connections(options.database_max_connections)
            .connect(options.database.expose())
            .await
            .context("error connecting to database")?;

        let version = pool
            .fetch_one("SELECT version()")
            .await
            .context("error getting database version")?
            .get::<String, _>(0);
        info!(url = %&options.database, ?version, "Connected to database");

        // Run migrations if requested.
        let latest = MIGRATOR
            .migrations
            .last()
            .expect("Missing migrations")
            .version;

        if options.database_migrate {
            info!(url = %&options.database, "Running migrations");
            MIGRATOR.run(&pool).await?;
        }

        // Validate database schema version
        #[allow(deprecated)] // HACK: No good alternative to `version()`?
        if let Some((version, dirty)) = pool.acquire().await?.version().await? {
            if dirty {
                error!(
                    url = %&options.database,
                    version,
                    expected = latest,
                    "Database is in incomplete migration state.",
                );
                return Err(anyhow!("Database is in incomplete migration state."));
            } else if version < latest {
                error!(
                    url = %&options.database,
                    version,
                    expected = latest,
                    "Database is not up to date, try rerunning with --database-migrate",
                );
                return Err(anyhow!(
                    "Database is not up to date, try rerunning with --database-migrate"
                ));
            } else if version > latest {
                error!(
                    url = %&options.database,
                    version,
                    latest,
                    "Database version is newer than this version of the software, please update.",
                );
                return Err(anyhow!(
                    "Database version is newer than this version of the software, please update."
                ));
            }
            info!(
                url = %&options.database,
                version,
                latest,
                "Database version is up to date.",
            );
        } else {
            error!(url = %&options.database, "Could not get database version");
            return Err(anyhow!("Could not get database version."));
        }

        Ok(Self { pool })
    }

    pub async fn insert_pending_identity(
        &self,
        leaf_index: usize,
        identity: &Hash,
        root: &Hash,
    ) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;

        let insert_pending_identity_query = sqlx::query(
            r#"
            INSERT INTO identities (leaf_index, commitment, root, status, pending_as_of)
            VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP)
            ON CONFLICT (root) DO NOTHING;
            "#,
        )
        .bind(leaf_index as i64)
        .bind(identity)
        .bind(root)
        .bind(<&str>::from(Status::Pending));

        tx.execute(insert_pending_identity_query).await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn get_id_by_root(
        tx: impl Executor<'_, Database = Postgres>,
        root: &Hash,
    ) -> Result<Option<usize>, Error> {
        let root_index_query = sqlx::query(
            r#"
            SELECT id FROM identities WHERE root = $1
            "#,
        )
        .bind(root);

        let row = tx.fetch_optional(root_index_query).await?;

        let Some(row) = row else { return Ok(None) };
        let root_id = row.get::<i64, _>(0);

        Ok(Some(root_id as usize))
    }

    /// Marks the identities and roots from before a given root hash as mined
    /// Also marks following roots as pending
    #[instrument(skip(self), level = "debug")]
    pub async fn mark_root_as_processed(&self, root: &Hash) -> Result<(), Error> {
        let mined_status = Status::Mined;
        let processed_status = Status::Processed;
        let pending_status = Status::Pending;

        let mut tx = self.pool.begin().await?;

        let root_id = Self::get_id_by_root(&mut tx, root).await?;

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
            AND    status <> $3
            "#,
        )
        .bind(root_id)
        .bind(<&str>::from(processed_status))
        .bind(<&str>::from(mined_status));

        let update_next_roots = sqlx::query(
            r#"
            UPDATE identities
            SET    status = $2, mined_at = NULL
            WHERE  id > $1
            "#,
        )
        .bind(root_id)
        .bind(<&str>::from(pending_status));

        tx.execute(update_previous_roots).await?;
        tx.execute(update_next_roots).await?;

        tx.commit().await?;

        Ok(())
    }

    /// Marks the identities and roots from before a given root hash as
    /// finalized
    #[instrument(skip(self), level = "debug")]
    pub async fn mark_root_as_mined(&self, root: &Hash) -> Result<(), Error> {
        let mined_status = Status::Mined;

        let mut tx = self.pool.begin().await?;

        let root_id = Self::get_id_by_root(&mut tx, root).await?;

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

    pub async fn get_next_leaf_index(&self) -> Result<usize, Error> {
        let query = sqlx::query(
            r#"
            SELECT leaf_index FROM identities
            ORDER BY leaf_index DESC
            LIMIT 1
            "#,
        );

        let row = self.pool.fetch_optional(query).await?;

        let Some(row) = row else { return Ok(0) };
        let leaf_index = row.get::<i64, _>(0);

        Ok((leaf_index + 1) as usize)
    }

    pub async fn get_identity_leaf_index(
        &self,
        identity: &Hash,
    ) -> Result<Option<TreeItem>, Error> {
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

        let Some(row) = self.pool.fetch_optional(query).await? else {
            return Ok(None);
        };

        let leaf_index = row.get::<i64, _>(0) as usize;

        let status = row
            .get::<&str, _>(1)
            .parse()
            .expect("Status is unreadable, database is corrupt");

        Ok(Some(TreeItem { status, leaf_index }))
    }

    pub async fn get_commitments_by_status(
        &self,
        status: Status,
    ) -> Result<Vec<TreeUpdate>, Error> {
        let query = sqlx::query(
            r#"
            SELECT leaf_index, commitment
            FROM identities
            WHERE status = $1
            ORDER BY leaf_index ASC, id ASC;
            "#,
        )
        .bind(<&str>::from(status));

        let rows = self.pool.fetch_all(query).await?;

        Ok(rows
            .into_iter()
            .map(|row| TreeUpdate {
                leaf_index: row.get::<i64, _>(0) as usize,
                element:    row.get::<Hash, _>(1),
            })
            .collect::<Vec<_>>())
    }

    pub async fn get_latest_root_by_status(&self, status: Status) -> Result<Option<Hash>, Error> {
        let query = sqlx::query(
            r#"
              SELECT root FROM identities WHERE status = $1 ORDER BY id DESC LIMIT 1
            "#,
        )
        .bind(<&str>::from(status));

        let row = self.pool.fetch_optional(query).await?;

        Ok(row.map(|r| r.get::<Hash, _>(0)))
    }

    pub async fn get_root_state(&self, root: &Hash) -> Result<Option<RootItem>, Error> {
        // This tries really hard to do everything in one query to prevent race
        // conditions.
        let query = sqlx::query(
            r#"
            SELECT
                status,
                pending_as_of as pending_valid_as_of,
                mined_at as mined_valid_as_of
            FROM identities
            WHERE root = $1;
            "#,
        )
        .bind(root);

        let row = self.pool.fetch_optional(query).await?;

        Ok(row.map(|r| {
            let status = r
                .get::<&str, _>(0)
                .parse()
                .expect("Status is unreadable, database is corrupt");

            let pending_valid_as_of = r.get::<_, _>(1);
            let mined_valid_as_of = r.get::<_, _>(2);

            RootItem {
                root: *root,
                status,
                pending_valid_as_of,
                mined_valid_as_of,
            }
        }))
    }

    pub async fn get_latest_insertion_timestamp(&self) -> Result<Option<DateTime<Utc>>, Error> {
        let query = sqlx::query(
            r#"
            SELECT insertion_timestamp
            FROM latest_insertion_timestamp
            WHERE Lock = 'X';"#,
        );

        let row = self.pool.fetch_optional(query).await?;

        Ok(row.map(|r| r.get::<DateTime<Utc>, _>(0)))
    }

    pub async fn count_unprocessed_identities(&self) -> Result<i32, Error> {
        let query = sqlx::query(
            r#"
            SELECT COUNT(*) as unprocessed
            FROM unprocessed_identities
            "#,
        );
        let result = self.pool.fetch_one(query).await?;
        Ok(result.get::<i64, _>(0) as i32)
    }

    pub async fn count_pending_identities(&self) -> Result<i32, Error> {
        let query = sqlx::query(
            r#"
            SELECT COUNT(*) as pending
            FROM identities
            WHERE status = $1
            "#,
        )
        .bind(<&str>::from(Status::Pending));
        let result = self.pool.fetch_one(query).await?;
        Ok(result.get::<i64, _>(0) as i32)
    }

    pub async fn get_provers(&self) -> Result<Provers, Error> {
        let query = sqlx::query(
            r#"
                SELECT batch_size, url, timeout_s, prover_type
                FROM provers
            "#,
        );

        let result = self.pool.fetch_all(query).await?;

        Ok(result
            .iter()
            .map(|row| {
                let batch_size = row.get::<i64, _>(0) as usize;
                let url = row.get::<String, _>(1);
                let timeout_s = row.get::<i64, _>(2) as u64;
                let prover_type = row.get::<ProverType, _>(3);
                ProverConfiguration {
                    url,
                    timeout_s,
                    batch_size,
                    prover_type,
                }
            })
            .collect::<Provers>())
    }

    pub async fn insert_prover_configuration(
        &self,
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

        self.pool.execute(query).await?;

        Ok(())
    }

    pub async fn insert_provers(&self, provers: HashSet<ProverConfiguration>) -> Result<(), Error> {
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

        self.pool.execute(query).await?;
        Ok(())
    }

    pub async fn remove_prover(
        &self,
        batch_size: usize,
        prover_type: ProverType,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
              DELETE FROM provers WHERE batch_size = $1 AND prover_type = $2
            "#,
        )
        .bind(batch_size as i64)
        .bind(prover_type);

        self.pool.execute(query).await?;

        Ok(())
    }

    pub async fn insert_new_identity(
        &self,
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
        .bind(<&str>::from(PendingStatus::New))
        .bind(eligibility_timestamp);

        self.pool.execute(query).await?;
        Ok(identity)
    }

    pub async fn insert_new_recovery(
        &self,
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
        self.pool.execute(query).await?;
        Ok(())
    }

    pub async fn get_latest_deletion(&self) -> Result<LatestDeletionEntry, Error> {
        let query =
            sqlx::query("SELECT deletion_timestamp FROM latest_deletion_root WHERE Lock = 'X';");

        let row = self.pool.fetch_optional(query).await?;

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

    pub async fn update_latest_insertion_timestamp(
        &self,
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

        self.pool.execute(query).await?;
        Ok(())
    }

    pub async fn update_latest_deletion(
        &self,
        deletion_timestamp: DateTime<Utc>,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO latest_deletion_root (Lock, deletion_timestamp)
            VALUES ('X', $1)
            ON CONFLICT (Lock)
            DO UPDATE SET deletion_timestamp = EXCLUDED.deletion_timestamp;
            "#,
        )
        .bind(deletion_timestamp);

        self.pool.execute(query).await?;
        Ok(())
    }

    // TODO: consider using a larger value than i64 for leaf index, ruint should
    // have postgres compatibility for u256
    pub async fn get_recoveries(&self) -> Result<Vec<RecoveryEntry>, Error> {
        let query = sqlx::query(
            r#"
            SELECT *
            FROM recoveries
            "#,
        );

        let result = self.pool.fetch_all(query).await?;

        Ok(result
            .into_iter()
            .map(|row| RecoveryEntry {
                existing_commitment: row.get::<Hash, _>(0),
                new_commitment:      row.get::<Hash, _>(1),
            })
            .collect::<Vec<RecoveryEntry>>())
    }

    pub async fn insert_new_deletion(
        &self,
        leaf_index: usize,
        identity: &Hash,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
            INSERT INTO deletions (leaf_index, commitment)
            VALUES ($1, $2)
            "#,
        )
        .bind(leaf_index as i64)
        .bind(identity);

        self.pool.execute(query).await?;
        Ok(())
    }

    // TODO: consider using a larger value than i64 for leaf index, ruint should
    // have postgres compatibility for u256
    pub async fn get_deletions(&self) -> Result<Vec<DeletionEntry>, Error> {
        let query = sqlx::query(
            r#"
            SELECT *
            FROM deletions
            "#,
        );

        let result = self.pool.fetch_all(query).await?;

        Ok(result
            .into_iter()
            .map(|row| DeletionEntry {
                leaf_index: row.get::<i64, _>(0) as usize,
                commitment: row.get::<Hash, _>(1),
            })
            .collect::<Vec<DeletionEntry>>())
    }

    /// Remove a list of entries from the deletions table
    pub async fn remove_deletions(&self, commitments: Vec<Hash>) -> Result<(), Error> {
        let placeholders: String = commitments
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect::<Vec<String>>()
            .join(", ");

        let query = format!(
            "DELETE FROM deletions WHERE commitment IN ({})",
            placeholders
        );

        let mut query = sqlx::query(&query);

        for commitment in &commitments {
            query = query.bind(commitment);
        }

        query.execute(&self.pool).await?;

        Ok(())
    }

    pub async fn get_eligible_unprocessed_commitments(
        &self,
        status: PendingStatus,
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

        let result = self.pool.fetch_all(query).await?;

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

    pub async fn get_unprocessed_commit_status(
        &self,
        commitment: &Hash,
    ) -> Result<Option<(PendingStatus, String)>, Error> {
        let query = sqlx::query(
            r#"
                SELECT status, error_message FROM unprocessed_identities WHERE commitment = $1
            "#,
        )
        .bind(commitment);

        let result = self.pool.fetch_optional(query).await?;

        if let Some(row) = result {
            return Ok(Some((
                row.get::<&str, _>(0).parse().expect("couldn't read status"),
                row.get::<Option<String>, _>(1).unwrap_or_default(),
            )));
        };
        Ok(None)
    }

    pub async fn remove_unprocessed_identity(&self, commitment: &Hash) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
                DELETE FROM unprocessed_identities WHERE commitment = $1
            "#,
        )
        .bind(commitment);

        self.pool.execute(query).await?;

        Ok(())
    }

    pub async fn update_err_unprocessed_commitment(
        &self,
        commitment: Hash,
        message: String,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"
                UPDATE unprocessed_identities SET error_message = $1, status = $2
                WHERE commitment = $3
            "#,
        )
        .bind(message)
        .bind(<&str>::from(PendingStatus::Failed))
        .bind(commitment);

        self.pool.execute(query).await?;

        Ok(())
    }

    pub async fn identity_exists(&self, commitment: Hash) -> Result<bool, Error> {
        let query_unprocessed_identity = sqlx::query(
            r#"SELECT exists(SELECT 1 FROM unprocessed_identities where commitment = $1)"#,
        )
        .bind(commitment);

        let row_unprocessed = self.pool.fetch_one(query_unprocessed_identity).await?;

        let query_processed_identity =
            sqlx::query(r#"SELECT exists(SELECT 1 FROM identities where commitment = $1)"#)
                .bind(commitment);

        let row_processed = self.pool.fetch_one(query_processed_identity).await?;

        let exists = row_unprocessed.get::<bool, _>(0) || row_processed.get::<bool, _>(0);

        Ok(exists)
    }

    // TODO: add docs
    pub async fn identity_is_queued_for_deletion(&self, commitment: &Hash) -> Result<bool, Error> {
        let query_queued_deletion =
            sqlx::query(r#"SELECT exists(SELECT 1 FROM deletions where commitment = $1)"#)
                .bind(commitment);
        let row_unprocessed = self.pool.fetch_one(query_queued_deletion).await?;
        Ok(row_unprocessed.get::<bool, _>(0))
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
    use postgres_docker_utils::DockerContainerGuard;
    use ruint::Uint;
    use semaphore::Field;

    use super::{Database, Options};
    use crate::identity_tree::{Hash, PendingStatus, Status};
    use crate::prover::{ProverConfiguration, ProverType};
    use crate::secret::SecretUrl;

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
    async fn setup_db() -> anyhow::Result<(Database, DockerContainerGuard)> {
        let db_container = postgres_docker_utils::setup().await?;
        let db_socket_addr = db_container.address();
        let url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

        let db = Database::new(Options {
            database:                 SecretUrl::from_str(&url)?,
            database_migrate:         true,
            database_max_connections: 1,
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
        expected_state: Status,
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
        let (db, _db_container) = setup_db().await?;
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
        assert_eq!(commit.0, PendingStatus::New);

        let identity_count = db
            .get_eligible_unprocessed_commitments(PendingStatus::New)
            .await?
            .len();

        assert_eq!(identity_count, 1);

        assert!(db.remove_unprocessed_identity(&commit_hash).await.is_ok());

        Ok(())
    }

    #[tokio::test]
    async fn insert_and_delete_identity() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

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

    fn mock_provers() -> HashSet<ProverConfiguration> {
        let mut provers = HashSet::new();

        provers.insert(ProverConfiguration {
            batch_size:  100,
            url:         "http://localhost:8080".to_string(),
            timeout_s:   100,
            prover_type: ProverType::Insertion,
        });

        provers.insert(ProverConfiguration {
            batch_size:  100,
            url:         "http://localhost:8080".to_string(),
            timeout_s:   100,
            prover_type: ProverType::Deletion,
        });

        provers
    }

    #[tokio::test]
    async fn test_insert_prover_configuration() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let mock_prover_configuration_0 = ProverConfiguration {
            batch_size:  100,
            url:         "http://localhost:8080".to_string(),
            timeout_s:   100,
            prover_type: ProverType::Insertion,
        };

        let mock_prover_configuration_1 = ProverConfiguration {
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
        let (db, _db_container) = setup_db().await?;
        let mock_provers = mock_provers();

        db.insert_provers(mock_provers.clone()).await?;

        let provers = db.get_provers().await?;

        assert_eq!(provers, mock_provers);
        Ok(())
    }

    #[tokio::test]
    async fn test_remove_prover() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let mock_provers = mock_provers();

        db.insert_provers(mock_provers.clone()).await?;
        db.remove_prover(100, ProverType::Insertion).await?;
        let provers = db.get_provers().await?;

        assert_eq!(provers, HashSet::new());

        Ok(())
    }

    #[tokio::test]
    async fn test_insert_new_recovery() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let existing_commitment: Uint<256, 4> = Uint::from(1);
        let new_commitment: Uint<256, 4> = Uint::from(2);

        db.insert_new_recovery(&existing_commitment, &new_commitment)
            .await?;

        let recoveries = db.get_recoveries().await?;

        assert_eq!(recoveries.len(), 1);
        assert_eq!(recoveries[0].existing_commitment, existing_commitment);
        assert_eq!(recoveries[0].new_commitment, new_commitment);

        Ok(())
    }

    #[tokio::test]
    async fn test_insert_new_deletion() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;
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
        let (db, _db_container) = setup_db().await?;
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
            .get_eligible_unprocessed_commitments(PendingStatus::New)
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
        let (db, _db_container) = setup_db().await?;

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
            .get_eligible_unprocessed_commitments(PendingStatus::New)
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
        let (db, _db_container) = setup_db().await?;
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
        let (db, _db_container) = setup_db().await?;
        let dec = "1234500000000000000";
        let commit_hash: Hash = U256::from_dec_str(dec)
            .expect("cant convert to u256")
            .into();

        // Set eligibility to Utc::now() day and check db entries
        let eligibility_timestamp = Utc::now();
        db.insert_new_identity(commit_hash, eligibility_timestamp)
            .await?;

        let commitments = db
            .get_eligible_unprocessed_commitments(PendingStatus::New)
            .await?;
        assert_eq!(commitments.len(), 1);

        let eligible_commitments = db
            .get_eligible_unprocessed_commitments(PendingStatus::New)
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
            .get_eligible_unprocessed_commitments(PendingStatus::New)
            .await?;
        assert_eq!(eligible_commitments.len(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_update_insertion_timestamp() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let insertion_timestamp = Utc::now();

        db.update_latest_insertion_timestamp(insertion_timestamp)
            .await?;

        let latest_insertion_timestamp = db.get_latest_insertion_timestamp().await?.unwrap();

        assert!(latest_insertion_timestamp.timestamp() - insertion_timestamp.timestamp() <= 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_insert_deletion() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;
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
        let (db, _db_container) = setup_db().await?;

        let old_identities = mock_identities(3);
        let new_identities = mock_identities(3);

        for (old, new) in old_identities.into_iter().zip(new_identities) {
            db.insert_new_recovery(&old, &new).await?;
        }

        let recoveries = db.get_recoveries().await?;
        assert_eq!(recoveries.len(), 3);

        Ok(())
    }

    #[tokio::test]
    async fn get_last_leaf_index() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

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
    async fn mark_root_as_processed_marks_previous_roots() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

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

            assert_eq!(root.status, Status::Processed);
        }

        for root in roots.iter().skip(3).take(2) {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, Status::Pending);
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
        let (db, _db_container) = setup_db().await?;

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

            assert_eq!(root.status, Status::Mined);
        }

        for root in roots.iter().skip(3).take(2) {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, Status::Pending);
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
        let (db, _db_container) = setup_db().await?;

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

        assert_roots_are(&db, &roots[..3], Status::Processed).await?;
        assert_roots_are(&db, &roots[3..], Status::Pending).await?;

        println!("Marking roots up to 1st as mined");
        db.mark_root_as_mined(&roots[1]).await?;

        assert_roots_are(&db, &roots[..2], Status::Mined).await?;
        assert_roots_are(&db, &[roots[2]], Status::Processed).await?;
        assert_roots_are(&db, &roots[3..], Status::Pending).await?;

        println!("Marking roots up to 4th as processed");
        db.mark_root_as_processed(&roots[4]).await?;

        assert_roots_are(&db, &roots[..2], Status::Mined).await?;
        assert_roots_are(&db, &roots[2..5], Status::Processed).await?;
        assert_roots_are(&db, &roots[5..], Status::Pending).await?;

        println!("Marking all roots as mined");
        db.mark_root_as_mined(&roots[num_identities - 1]).await?;

        assert_roots_are(&db, &roots, Status::Mined).await?;

        Ok(())
    }

    #[tokio::test]
    async fn mark_root_as_processed_marks_next_roots() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

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

            assert_eq!(root.status, Status::Processed);
        }

        for root in roots.iter().skip(2).take(3) {
            let root = db
                .get_root_state(root)
                .await?
                .context("Fetching root state")?;

            assert_eq!(root.status, Status::Pending);
        }

        let pending_identities = db.count_pending_identities().await?;
        assert_eq!(pending_identities, 3, "There should be 3 pending roots");

        Ok(())
    }

    #[tokio::test]
    async fn root_history_timing() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

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
        let (db, _db_container) = setup_db().await?;

        let identities = mock_identities(5);

        let roots = mock_roots(7);

        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], &roots[i])
                .await
                .context("Inserting identity")?;
        }

        db.mark_root_as_processed(&roots[2]).await?;

        let mined_tree_updates = db.get_commitments_by_status(Status::Processed).await?;
        let pending_tree_updates = db.get_commitments_by_status(Status::Pending).await?;

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
    async fn get_commitments_by_status_results_are_not_deduplicated() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

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

        let pending_tree_updates = db.get_commitments_by_status(Status::Pending).await?;
        assert_eq!(pending_tree_updates.len(), 7);

        // 1st identity
        assert_eq!(
            pending_tree_updates[0].element, identities[0],
            "First element is the original value"
        );
        assert_eq!(
            pending_tree_updates[1].element,
            Hash::ZERO,
            "Second element is the updated (deleted) value"
        );

        // 3rd identity
        assert_eq!(
            pending_tree_updates[4].element, identities[3],
            "First element is the original value"
        );
        assert_eq!(
            pending_tree_updates[5].element,
            Hash::ZERO,
            "Second element is the updated (deleted) value"
        );

        Ok(())
    }

    #[tokio::test]
    async fn get_commitments_by_status_results_are_sorted() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let identities = mock_identities(5);

        let roots = mock_roots(5);

        let unordered_indexes = vec![3, 1, 4, 2, 0];
        for i in unordered_indexes {
            db.insert_pending_identity(i, &identities[i], &roots[i])
                .await
                .context("Inserting identity")?;
        }

        let pending_tree_updates = db.get_commitments_by_status(Status::Pending).await?;
        assert_eq!(pending_tree_updates.len(), 5);
        for i in 0..5 {
            assert_eq!(pending_tree_updates[i].element, identities[i]);
            assert_eq!(pending_tree_updates[i].leaf_index, i);
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_root_invalidation() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

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
        assert!(matches!(root_item.status, Status::Pending));
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
        assert!(matches!(root_item_2.status, Status::Pending));
        assert!(root_item_2.mined_valid_as_of.is_none());

        let root_item_1 = db.get_root_state(&roots[1]).await?.unwrap();
        assert_eq!(root_item_1.status, Status::Pending);
        assert!(root_item_1.mined_valid_as_of.is_none());
        assert!(root_item_1.pending_valid_as_of < root_2_mined_at);

        let root_item_0 = db.get_root_state(&roots[0]).await?.unwrap();
        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);
        assert_eq!(root_item_0.status, Status::Processed);
        assert!(root_item_0.mined_valid_as_of.unwrap() < root_2_mined_at);
        assert!(root_item_0.mined_valid_as_of.unwrap() > root_1_inserted_at);
        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);

        Ok(())
    }

    #[tokio::test]
    async fn check_identity_existence() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

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
        let (db, _db_container) = setup_db().await?;

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
        db.remove_deletions(identities[0..=2].to_vec()).await?;
        let deletions = db.get_deletions().await?;

        assert_eq!(deletions.len(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_latest_deletion_root() -> anyhow::Result<()> {
        let (db, _db_container) = setup_db().await?;

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
}
