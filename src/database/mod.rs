#![allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]

use anyhow::{anyhow, Context, Error as ErrReport};
use clap::Parser;
use sqlx::{
    any::AnyKind,
    migrate::{Migrate, MigrateDatabase, Migrator},
    pool::PoolOptions,
    Any, Executor, Pool, Row,
};
use thiserror::Error;
use tracing::{error, info, instrument, warn};
use url::Url;

use crate::identity_tree::{Hash, RootItem, Status, TreeItem, TreeUpdate};

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

    pub async fn insert_identity_if_does_not_exist(
        &self,
        identity: &Hash,
    ) -> Result<Option<usize>, Error> {
        let query = sqlx::query(
            r#"INSERT INTO identities (commitment, leaf_index, status)
                       VALUES ($1, (SELECT COALESCE(MAX(leaf_index) + 1, 0) FROM identities), $2)
                       ON CONFLICT DO NOTHING
                       RETURNING leaf_index;"#,
        )
        .bind(identity)
        .bind(<&str>::from(Status::Pending));
        let row = self.pool.fetch_optional(query).await?;
        Ok(row.map(|row| row.get::<i64, _>(0) as usize))
    }

    /// Marks the identities at the provided `leaf_indices` as being submitted
    /// to the contract on chain.
    ///
    /// # Note
    /// All updates are performed as part of a single transaction. If any
    /// failure occurs, the entire batch will be rolled back.
    pub async fn mark_identities_submitted_to_contract(
        &self,
        root: &Hash,
        leaf_indices: &[usize],
    ) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;

        // Note that there are more efficient ways to do this in certain dialects of
        // SQL. Postgres has the `VALUES` embedding, for example. Using a transaction as
        // done here is backend agnostic, and hence works in production and for the
        // tests.
        for index in leaf_indices {
            let query = sqlx::query(
                r#"UPDATE identities
                           SET status = $2
                           WHERE leaf_index = $1;"#,
            )
            .bind(*index as i64)
            .bind(<&str>::from(Status::Mined));
            tx.execute(query).await?;
        }

        let query = sqlx::query(
            r#"UPDATE root_history
            SET status = $2, mined_at = CURRENT_TIMESTAMP
            WHERE root = $1;"#,
        )
        .bind(root)
        .bind(<&str>::from(Status::Mined));
        tx.execute(query).await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn get_updates_in_range(
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

    pub async fn get_identity_leaf_index(
        &self,
        identity: &Hash,
    ) -> Result<Option<TreeItem>, Error> {
        let query = sqlx::query(
            r#"SELECT leaf_index, status
                       FROM identities
                       WHERE commitment = $1
                       LIMIT 1;"#,
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
            r#"SELECT leaf_index, commitment
                       FROM identities
                       WHERE status = $1
                       ORDER BY leaf_index ASC;"#,
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

    pub async fn get_root_state(&self, root: &Hash) -> Result<Option<RootItem>, Error> {
        // This tries really hard to do everything in one query to prevent race
        // conditions.
        let query = sqlx::query(
            r#"SELECT
                status,
                COALESCE(
                    (SELECT r2.created_at
                    FROM root_history r2
                    WHERE r2.last_index > r.last_index
                    ORDER BY r2.last_index
                    LIMIT 1),
                    CURRENT_TIMESTAMP
                ) as pending_valid_as_of,
                COALESCE(
                    (SELECT r2.mined_at
                    FROM root_history r2
                    WHERE r2.last_index > r.last_index AND r2.status = $2
                    ORDER BY r2.last_index
                    LIMIT 1),
                    CASE WHEN r.status = $2 THEN CURRENT_TIMESTAMP END
                ) as mined_valid_as_of
                FROM root_history r
                WHERE r.root = $1;
            "#,
        )
        .bind(root)
        .bind(<&str>::from(Status::Mined));

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

    pub async fn insert_pending_root(
        &self,
        root: &Hash,
        last_identity: &Hash,
        last_index: usize,
    ) -> Result<(), Error> {
        let query = sqlx::query(
            r#"INSERT INTO
            root_history(root, last_identity, last_index, status, created_at)
            VALUES($1, $2, $3, $4, CURRENT_TIMESTAMP)
            ON CONFLICT (root) DO NOTHING;"#,
        )
        .bind(root)
        .bind(last_identity)
        .bind(last_index as i64)
        .bind(<&str>::from(Status::Pending));
        self.pool.execute(query).await?;

        Ok(())
    }

    pub async fn count_pending_identities(&self) -> Result<i32, Error> {
        let query =
            sqlx::query(r#"SELECT COUNT(leaf_index) as pending FROM identities WHERE status = $1"#)
                .bind(<&str>::from(Status::Pending));
        let result = self.pool.fetch_one(query).await?;
        Ok(result.get::<i64, _>(0) as i32)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    InternalError(#[from] sqlx::Error),
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use chrono::Utc;
    use clap::Parser;
    use semaphore::Field;

    use crate::identity_tree::Status;

    use super::{Database, Options};

    #[tokio::test]
    async fn test_root_invalidation() {
        let db = Database::new(Options::try_parse_from([""]).unwrap())
            .await
            .unwrap();

        let identities = (1..5).map(Field::from).collect::<Vec<_>>();
        let roots = (1..5).map(Field::from).collect::<Vec<_>>();
        db.insert_identity_if_does_not_exist(&identities[0])
            .await
            .unwrap();
        db.insert_pending_root(&roots[0], &identities[0], 0)
            .await
            .unwrap();

        // Invalid root returns None
        assert!(db.get_root_state(&roots[1]).await.unwrap().is_none());

        // Basic scenario, latest pending root
        let root_item = db.get_root_state(&roots[0]).await.unwrap().unwrap();
        assert_eq!(roots[0], root_item.root);
        assert!(matches!(root_item.status, Status::Pending));
        assert!(root_item.mined_valid_as_of.is_none());

        // Inserting a new pending root sets invalidation time for the previous root
        db.insert_identity_if_does_not_exist(&identities[1])
            .await
            .unwrap();
        db.insert_identity_if_does_not_exist(&identities[2])
            .await
            .unwrap();
        db.insert_pending_root(&roots[1], &identities[2], 2)
            .await
            .unwrap();
        let root_1_inserted_at = Utc::now();

        tokio::time::sleep(Duration::from_secs(2)).await; // sleep enough for SQLite time resolution

        let root_item_0 = db.get_root_state(&roots[0]).await.unwrap().unwrap();
        let root_item_1 = db.get_root_state(&roots[1]).await.unwrap().unwrap();

        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);
        assert!(root_item_1.pending_valid_as_of > root_1_inserted_at);

        // Test mined roots
        db.insert_identity_if_does_not_exist(&identities[3])
            .await
            .unwrap();
        db.insert_pending_root(&roots[2], &identities[3], 3)
            .await
            .unwrap();
        db.mark_identities_submitted_to_contract(&roots[2], &[0, 1, 2, 3])
            .await
            .unwrap();
        let root_2_mined_at = Utc::now();

        tokio::time::sleep(Duration::from_secs(2)).await; // sleep enough for SQLite time resolution

        let root_item_2 = db.get_root_state(&roots[2]).await.unwrap().unwrap();
        assert!(matches!(root_item_2.status, Status::Mined));
        assert!(root_item_2.mined_valid_as_of.is_some());

        let root_item_1 = db.get_root_state(&roots[1]).await.unwrap().unwrap();
        assert!(matches!(root_item_1.status, Status::Pending));
        assert!(root_item_1.mined_valid_as_of.unwrap() < root_2_mined_at);
        assert!(root_item_1.pending_valid_as_of < root_2_mined_at);

        let root_item_0 = db.get_root_state(&roots[0]).await.unwrap().unwrap();
        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);
        assert!(matches!(root_item_0.status, Status::Pending));
        assert!(root_item_0.mined_valid_as_of.unwrap() < root_2_mined_at);
        assert!(root_item_0.mined_valid_as_of.unwrap() > root_1_inserted_at);
        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);
    }
}
