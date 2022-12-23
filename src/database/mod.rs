use crate::identity_tree::Hash;
use anyhow::{anyhow, Context, Error as ErrReport};
use clap::Parser;
use ruint::{aliases::U256, uint};
use semaphore::Field;
use sqlx::{
    any::AnyKind,
    migrate::{Migrate, MigrateDatabase, Migrator},
    pool::PoolOptions,
    Any, Executor, Pool, Row,
};
use thiserror::Error;
use tracing::{error, info, instrument, warn};
use url::Url;

mod sitter;
pub use sitter::InsertTxError;

// Statically link in migration files
static MIGRATOR: Migrator = sqlx::migrate!("schemas/database");

#[derive(Clone, Debug, PartialEq, Eq, Parser)]
pub struct Options {
    /// Database server connection string.
    /// Example: `postgres://user:password@localhost:5432/database`
    /// Sqlite file: ``
    /// In memory DB: `sqlite::memory:`
    #[clap(long, env, default_value = "sqlite::memory:")]
    pub database: Url,

    /// Allow creation or migration of the database schema.
    #[clap(long, default_value = "true")]
    pub database_migrate: bool,

    /// Maximum number of connections in the database connection pool
    #[clap(long, env, default_value = "10")]
    pub database_max_connections: u32,
}

pub struct Database {
    pool: Pool<Any>,
}

impl Database {
    #[instrument(skip_all)]
    pub async fn new(options: Options) -> Result<Self, ErrReport> {
        info!(url = %&options.database, "Connecting to database");

        // Create database if requested and does not exist
        if options.database_migrate && !Any::database_exists(options.database.as_str()).await? {
            warn!(url = %&options.database, "Database does not exist, creating
        database");
            Any::create_database(options.database.as_str()).await?;
        }

        // Create a connection pool
        let pool = PoolOptions::<Any>::new()
            .max_connections(options.database_max_connections)
            .connect(options.database.as_str())
            .await
            .context("error connecting to database")?;

        // Log DB version to test connection.
        let sql = match pool.any_kind() {
            AnyKind::Sqlite => "sqlite_version() || ' ' || sqlite_source_id()",
            AnyKind::Postgres => "version()",

            // Depending on compilation flags there may be more patterns.
            #[allow(unreachable_patterns)]
            _ => "'unknown'",
        };
        let version = pool
            .fetch_one(format!("SELECT {sql};").as_str())
            .await
            .context("error getting database version")?
            .get::<String, _>(0);
        info!(url = %&options.database, kind = ?pool.any_kind(), ?version, "Connected to database");

        // Run migrations if requested.
        let latest = MIGRATOR.migrations.last().unwrap().version;
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
        group_id: usize,
        identity: &Hash,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"INSERT INTO pending_identities (group_id, commitment)
                   VALUES ($1, $2);"#,
        )
        .bind(group_id as i64)
        .bind(identity);
        self.pool.execute(query).await?;
        Ok(())
    }

    pub async fn mark_identity_inserted(
        &self,
        group_id: usize,
        commitment: &Hash,
        block_number: usize,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"UPDATE pending_identities
                   SET mined_in_block = $1
                   WHERE group_id = $2 AND commitment = $3;"#,
        )
        .bind(block_number as i64)
        .bind(group_id as i64)
        .bind(commitment);

        self.pool.execute(query).await?;
        Ok(())
    }

    pub async fn confirm_identity_and_retrigger_stale_recods(
        &self,
        commitment: &Hash,
    ) -> Result<IdentityConfirmationResult, Error> {
        let retrigger_query = sqlx::query(
            r#"UPDATE pending_identities
            SET mined_in_block = NULL, created_at = CURRENT_TIMESTAMP
            WHERE created_at < (SELECT created_at FROM pending_identities WHERE commitment = $1 LIMIT 1)"#,
        )
        .bind(commitment);

        let retrigger_result = self.pool.execute(retrigger_query).await?;

        let cleanup_query = sqlx::query(
            r#"DELETE FROM pending_identities
                WHERE commitment = $1;"#,
        )
        .bind(commitment);

        self.pool.execute(cleanup_query).await?;

        if retrigger_result.rows_affected() > 0 {
            Ok(IdentityConfirmationResult::RetriggerProcessing)
        } else {
            Ok(IdentityConfirmationResult::Done)
        }
    }

    pub async fn pending_identity_exists(
        &self,
        group_id: usize,
        identity: &Hash,
    ) -> Result<bool, Error> {
        let query = sqlx::query(
            r#"SELECT 1
                   FROM pending_identities
                   WHERE group_id = $1 AND commitment = $2
                   LIMIT 1;"#,
        )
        .bind(group_id as i64)
        .bind(identity);
        let row = self.pool.fetch_optional(query).await?;
        Ok(row.is_some())
    }

    pub async fn get_oldest_unprocessed_identity(&self) -> Result<Option<(usize, Hash)>, Error> {
        let query = sqlx::query(
            r#"SELECT group_id, commitment
                   FROM pending_identities
                   WHERE mined_in_block IS NULL
                   ORDER BY created_at ASC
                   LIMIT 1;"#,
        );
        let row = self.pool.fetch_optional(query).await?;
        Ok(row.map(|row| (row.get::<i64, _>(0).try_into().unwrap(), row.get(1))))
    }

    #[allow(unused)]
    pub async fn read(&self, _index: usize) -> Result<Hash, Error> {
        self.pool
            .execute(sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS hashes (
                id SERIAL PRIMARY KEY,
                hash TEXT NOT NULL
            );"#,
            ))
            .await?;

        let value = uint!(0x12356_U256);

        self.pool
            .execute(sqlx::query(r#"INSERT INTO hashes ( hash ) VALUES ( $1 );"#).bind(value))
            .await?;

        let rows = self
            .pool
            .fetch_all(sqlx::query(r#"SELECT hash FROM hashes;"#))
            .await?;
        for row in rows {
            let hash = row.get::<U256, _>(0);
            info!(hash = ?hash, "Read hash");
        }

        Ok(Hash::default())
    }

    pub async fn get_block_number(&self) -> Result<u64, Error> {
        let row = self
            .pool
            .fetch_optional(sqlx::query(
                r#"SELECT block_index FROM logs ORDER BY block_index DESC LIMIT 1;"#,
            ))
            .await?;

        if let Some(row) = row {
            let block_number: i64 = row.try_get(0)?;
            Ok(u64::try_from(block_number).unwrap_or(0))
        } else {
            Ok(0)
        }
    }

    pub async fn load_logs(
        &self,
        from_block: i64,
        to_block: Option<i64>,
    ) -> Result<Vec<(Field, Field)>, Error> {
        let rows = self
            .pool
            .fetch_all(
                sqlx::query(
                r#"SELECT leaf, root FROM logs WHERE block_index >= $1 AND block_index <= $2 ORDER BY block_index, transaction_index, log_index;"#,
                )
                .bind(from_block)
                .bind(to_block.unwrap_or(i64::MAX))
            )
            .await?
            .iter()
            .map(|row| (row.try_get(0).unwrap_or_default(), row.try_get(1).unwrap_or_default()))
            .collect();

        Ok(rows)
    }

    pub async fn save_log(&self, identity: &ConfirmedIdentityEvent) -> Result<(), Error> {
        self.pool
            .execute(
                sqlx::query(
                    r#"INSERT INTO logs (block_index, transaction_index, log_index, raw, leaf, root)
                    VALUES ($1, $2, $3, $4, $5, $6);"#,
                )
                .bind(identity.block_index)
                .bind(identity.transaction_index)
                .bind(identity.log_index)
                .bind(identity.raw_log.clone())
                .bind(identity.leaf)
                .bind(identity.root),
            )
            .await
            .map_err(Error::InternalError)?;

        Ok(())
    }

    pub async fn wipe_cache(&self) -> Result<(), Error> {
        self.pool
            .execute(sqlx::query("DELETE FROM logs;"))
            .await
            .map_err(Error::InternalError)?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error")]
    InternalError(#[from] sqlx::Error),
}

pub enum IdentityConfirmationResult {
    Done,
    RetriggerProcessing,
}

pub struct ConfirmedIdentityEvent {
    pub block_index:       i64,
    pub transaction_index: i32,
    pub log_index:         i32,
    pub raw_log:           String,
    pub leaf:              Field,
    pub root:              Field,
}
