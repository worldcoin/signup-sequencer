use crate::app::Hash;
use eyre::{Context, Result, WrapErr};
use sqlx::{any::AnyKind, pool::PoolOptions, Any, Executor, Pool, Row};
use structopt::StructOpt;
use tracing::{debug, info};
use url::Url;

#[derive(Clone, Debug, PartialEq, StructOpt)]
pub struct Options {
    /// Database server connection string
    #[structopt(
        long,
        env,
        default_value = "postgres://postgres:password@localhost/test"
    )]
    pub database: Url,

    /// Maximum number of connections in the database connection pool
    #[structopt(long, env, default_value = "10")]
    pub database_max_connections: u32,
}

pub struct Db {
    pool: Pool<Any>,
}

impl Db {
    pub async fn new(options: &Options) -> Result<Self> {
        debug!(url = %&options.database, "Connecting to database");

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

            _ => "'unknown'",
        };
        let version = pool
            .fetch_one(format!("SELECT {sql};", sql = sql).as_str())
            .await
            .wrap_err("error getting database version")?
            .get::<String, _>(0);
        info!(url = %&options.database, kind = ?pool.any_kind(), ?version, "Connected to database");

        Ok(Self { pool })
    }

    pub async fn read(&self, index: usize) -> Hash {
        todo!()
    }
}
