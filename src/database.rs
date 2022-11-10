use crate::app::Hash;
use clap::Parser;
use eyre::{eyre, Context, ErrReport};
use ruint::{
    aliases::{U256, U64},
    uint,
};
use serde_json::value::RawValue;
use sqlx::{
    any::AnyKind,
    migrate::{Migrate, MigrateDatabase, Migrator},
    pool::PoolOptions,
    Any, Executor, Pool, Row,
};
use thiserror::Error;
use tracing::{error, info, instrument, warn};
use url::Url;

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
            .wrap_err("error connecting to database")?;

        // Log DB version to test connection.
        let sql = match pool.any_kind() {
            #[cfg(feature = "sqlite")]
            AnyKind::Sqlite => "sqlite_version() || ' ' || sqlite_source_id()",

            #[cfg(feature = "postgres")]
            AnyKind::Postgres => "version()",

            // Depending on compilation flags there may be more patterns.
            #[allow(unreachable_patterns)]
            _ => "'unknown'",
        };
        let version = pool
            .fetch_one(format!("SELECT {sql};").as_str())
            .await
            .wrap_err("error getting database version")?
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
                return Err(eyre!("Database is in incomplete migration state."));
            } else if version < latest {
                error!(
                    url = %&options.database,
                    version,
                    expected = latest,
                    "Database is not up to date, try rerunning with --database-migrate",
                );
                return Err(eyre!(
                    "Database is not up to date, try rerunning with --database-migrate"
                ));
            } else if version > latest {
                error!(
                    url = %&options.database,
                    version,
                    latest,
                    "Database version is newer than this version of the software, please update.",
                );
                return Err(eyre!(
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
            return Err(eyre!("Could not get database version."));
        }

        Ok(Self { pool })
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
            let block_number: U64 = row.try_get(0)?;
            Ok(block_number.into_limbs()[0])
        } else {
            Ok(0)
        }
    }

    pub async fn load_logs(&self) -> Result<Vec<Box<RawValue>>, Error> {
        let rows = self
            .pool
            .fetch_all(sqlx::query(
                r#"SELECT raw FROM logs ORDER BY block_index, transaction_index, log_index;"#,
            ))
            .await?
            .iter()
            .map(|row| RawValue::from_string(row.get(0)).unwrap())
            .collect();

        Ok(rows)
    }

    pub async fn save_log(
        &self,
        block_index: u64,
        transaction_index: u64,
        log_index: U256,
        log: Box<RawValue>,
    ) -> Result<(), Error> {
        self.pool
            .execute(
                sqlx::query(
                    r#"INSERT INTO logs (block_index, transaction_index, log_index, raw)
                    VALUES ($1, $2, $3, $4);"#,
                )
                .bind(U64::from(block_index))
                .bind(U64::from(transaction_index))
                .bind(log_index)
                .bind(log.get()),
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
