use crate::app::Hash;
use eyre::{Context, Result};
use ruint::{aliases::U256, uint};
use sqlx::{any::AnyKind, migrate::MigrateDatabase, pool::PoolOptions, Any, Executor, Pool, Row};
use structopt::StructOpt;
use tracing::{debug, info, instrument, warn};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq, StructOpt)]
pub struct Options {
    /// Database server connection string.
    /// Example: `postgres://user:password@localhost:5432/database`
    /// Sqlite file: ``
    /// In memory DB: `sqlite::memory:`
    #[structopt(
        long,
        env,
        default_value = "postgres://postgres:password@localhost/test"
    )]
    pub database: Url,

    /// Create the database schema if it does not exist.
    #[structopt(long)]
    pub database_create: bool,

    /// Maximum number of connections in the database connection pool
    #[structopt(long, env, default_value = "10")]
    pub database_max_connections: u32,
}

pub struct Database {
    pool: Pool<Any>,
}

impl Database {
    #[instrument(skip_all)]
    pub async fn new(options: Options) -> Result<Self> {
        info!(url = %&options.database, "Connecting to database");

        // Create database if requested and does not exist
        if options.database_create && !Any::database_exists(options.database.as_str()).await? {
            warn!(url = %&options.database, "Database does not exist, creating database");
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
            .fetch_one(format!("SELECT {sql};", sql = sql).as_str())
            .await
            .wrap_err("error getting database version")?
            .get::<String, _>(0);
        info!(url = %&options.database, kind = ?pool.any_kind(), ?version, "Connected to database");

        // TODO: Test schema version.

        Ok(Self { pool })
    }

    pub async fn read(&self, index: usize) -> Result<Hash> {
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
}
