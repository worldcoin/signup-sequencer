use crate::identity_tree::{Hash, TreeItem, TreeUpdate, ValidityScope};
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

    pub async fn insert_identity_if_not_duplicate(
        &self,
        identity: &Hash,
    ) -> Result<Option<usize>, Error> {
        let query = sqlx::query(
            r#"INSERT INTO identities (commitment, leaf_index, status)
                   VALUES ($1, (SELECT COALESCE(MAX(leaf_index), 0) FROM identities), $2)
                   ON CONFLICT DO NOTHING
                   RETURNING leaf_index;"#,
        )
        .bind(identity)
        .bind(<&str>::from(ValidityScope::Pending));
        let row = self.pool.fetch_optional(query).await?;
        Ok(row.map(|row| row.get::<i64, _>(0) as usize))
    }

    pub async fn mark_identity_submitted_to_contract(
        &self,
        identity: &Hash,
        leaf_index: usize,
    ) -> Result<(), Error> {
        todo!()
    }

    pub async fn get_updates_range(
        &self,
        from_index: usize,
        to_index: usize,
    ) -> Result<Vec<TreeUpdate>, Error> {
        let query = sqlx::query(
            r#"SELECT commitment, leaf_index, status
                       FROM identities
                       WHERE leaf_index >= $1 AND leaf_index <= $2
                       ORDER BY leaf_index ASC;"#,
        )
        .bind(from_index as i64)
        .bind(to_index as i64);
        let rows = self.pool.fetch_all(query).await?;
        rows.iter()
            .map(|row| {
                let element = row.get::<Hash, _>(0);
                let leaf_index = row.get::<i64, _>(1) as usize;
                Ok(TreeUpdate {
                    leaf_index,
                    element,
                })
            })
            .collect::<Result<Vec<_>, _>>()
    }

    pub async fn get_identity_index(&self, identity: &Hash) -> Result<Option<TreeItem>, Error> {
        let query = sqlx::query(
            r#"SELECT leaf_index
                   FROM identities
                   WHERE commitment = $1
                   LIMIT 1;"#,
        )
        .bind(identity);
        let row = self.pool.fetch_optional(query).await?;
        // let ret = Ok(row.map(|row| row.get::<i64, _>(0) as usize));
        todo!("Is it all?");
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    InternalError(#[from] sqlx::Error),
}
