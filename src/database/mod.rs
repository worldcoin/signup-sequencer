#![allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

use std::cmp::Ordering;
use std::ops::Deref;

use anyhow::{anyhow, Context, Error as ErrReport};
use sqlx::migrate::{Migrate, MigrateDatabase, Migrator};
use sqlx::pool::PoolOptions;
use sqlx::{Executor, Pool, Postgres, Row, Transaction};
use thiserror::Error;
use tracing::{error, info, instrument, warn};

use crate::config::DatabaseConfig;
use crate::identity_tree::Hash;

pub mod methods;
pub mod types;

// Statically link in migration files
static MIGRATOR: Migrator = sqlx::migrate!("schemas/database");

pub struct Database {
    pub pool: Pool<Postgres>,
}

/// Transaction isolation level
///
/// PG docs: https://www.postgresql.org/docs/current/transaction-iso.html
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationLevel {
    ReadUncommited,
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

impl Deref for Database {
    type Target = Pool<Postgres>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

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

    pub async fn begin_tx(
        &self,
        isolation_level: IsolationLevel,
    ) -> Result<Transaction<'static, Postgres>, Error> {
        let mut tx = self.begin().await?;

        match isolation_level {
            IsolationLevel::ReadUncommited => {
                tx.execute("SET TRANSACTION ISOLATION LEVEL READ UNCOMMITTED")
                    .await?;
            }
            IsolationLevel::ReadCommitted => {
                tx.execute("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
                    .await?;
            }
            IsolationLevel::RepeatableRead => {
                tx.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                    .await?;
            }
            IsolationLevel::Serializable => {
                tx.execute("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                    .await?;
            }
        }

        Ok(tx)
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
    use chrono::Utc;
    use ethers::types::U256;
    use postgres_docker_utils::DockerContainer;
    use ruint::Uint;
    use semaphore_rs::poseidon_tree::LazyPoseidonTree;
    use semaphore_rs::Field;
    use testcontainers::clients::Cli;

    use super::Database;
    use crate::config::DatabaseConfig;
    use crate::database::methods::DbMethods;
    use crate::database::types::BatchType;
    use crate::identity_tree::{Hash, ProcessedStatus};
    use crate::prover::identity::Identity;
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
    async fn setup_db(docker: &Cli) -> anyhow::Result<(Database, DockerContainer)> {
        let db_container = postgres_docker_utils::setup(docker).await?;
        let url = format!(
            "postgres://postgres:postgres@{}/database",
            db_container.address()
        );

        let db = Database::new(&DatabaseConfig {
            database: SecretUrl::from_str(&url)?,
            migrate: true,
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

        db.insert_unprocessed_identity(commit_hash).await?;

        let identity_count = db.get_unprocessed_identities().await?.len();

        assert_eq!(identity_count, 1);

        Ok(())
    }

    #[tokio::test]
    async fn trim_unprocessed_identities() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let identities = mock_identities(10);
        let roots = mock_roots(11);

        for identity in &identities {
            db.insert_unprocessed_identity(*identity).await?;
        }

        assert_eq!(
            db.count_unprocessed_identities().await? as usize,
            identities.len()
        );

        for (idx, identity) in identities.iter().copied().enumerate() {
            println!("idx = {idx}");
            println!("roots[idx] = {}", roots[idx]);
            println!("roots[idx + 1] = {}", roots[idx + 1]);

            db.insert_pending_identity(
                idx,
                &identity,
                Some(Utc::now()),
                &roots[idx + 1],
                &roots[idx],
            )
            .await?;
        }

        db.trim_unprocessed().await?;

        assert_eq!(db.count_unprocessed_identities().await?, 0);

        Ok(())
    }

    #[tokio::test]
    async fn insert_and_delete_identity() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let zero: Hash = U256::zero().into();
        let initial_root = LazyPoseidonTree::new(4, zero).root();
        let zero_root: Hash = U256::from_dec_str("6789")?.into();
        let root: Hash = U256::from_dec_str("54321")?.into();
        let commitment: Hash = U256::from_dec_str("12345")?.into();

        db.insert_pending_identity(0, &commitment, Some(Utc::now()), &root, &initial_root)
            .await?;
        db.insert_pending_identity(0, &zero, Some(Utc::now()), &zero_root, &root)
            .await?;

        let item = db
            .get_tree_item(&commitment)
            .await?
            .context("Missing identity")?;

        assert_eq!(item.leaf_index, 0);

        Ok(())
    }

    fn mock_provers() -> HashSet<ProverConfig> {
        let mut provers = HashSet::new();

        provers.insert(ProverConfig {
            batch_size: 100,
            url: "http://localhost:8080".to_string(),
            timeout_s: 100,
            prover_type: ProverType::Insertion,
        });

        provers.insert(ProverConfig {
            batch_size: 100,
            url: "http://localhost:8080".to_string(),
            timeout_s: 100,
            prover_type: ProverType::Deletion,
        });

        provers
    }

    #[tokio::test]
    async fn insert_prover_configuration() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let mock_prover_configuration_0 = ProverConfig {
            batch_size: 100,
            url: "http://localhost:8080".to_string(),
            timeout_s: 100,
            prover_type: ProverType::Insertion,
        };

        let mock_prover_configuration_1 = ProverConfig {
            batch_size: 100,
            url: "http://localhost:8081".to_string(),
            timeout_s: 100,
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
    async fn insert_provers() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let mock_provers = mock_provers();

        db.insert_provers(mock_provers.clone()).await?;

        let provers = db.get_provers().await?;

        assert_eq!(provers, mock_provers);
        Ok(())
    }

    #[tokio::test]
    async fn remove_prover() -> anyhow::Result<()> {
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
    async fn insert_new_deletion() -> anyhow::Result<()> {
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
    async fn get_unprocessed_commitments() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        // Insert new identity
        let commitment_0: Uint<256, 4> = Uint::from(1);
        db.insert_unprocessed_identity(commitment_0).await?;

        // Insert new identity
        let commitment_1: Uint<256, 4> = Uint::from(2);
        db.insert_unprocessed_identity(commitment_1).await?;

        let unprocessed_commitments = db.get_unprocessed_identities().await?;

        // Assert unprocessed commitments against expected values
        assert_eq!(unprocessed_commitments.len(), 2);
        assert_eq!(unprocessed_commitments[0].commitment, commitment_0);
        assert_eq!(unprocessed_commitments[1].commitment, commitment_1);

        Ok(())
    }

    #[tokio::test]
    async fn insert_deletion() -> anyhow::Result<()> {
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
    async fn get_last_leaf_index() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(1);
        let roots = mock_roots(1);

        let next_leaf_index = db.get_next_leaf_index().await?;

        assert_eq!(next_leaf_index, 0, "Db should contain not leaf indexes");

        db.insert_pending_identity(
            0,
            &identities[0],
            Some(Utc::now()),
            &roots[0],
            &initial_root,
        )
        .await?;

        let next_leaf_index = db.get_next_leaf_index().await?;
        assert_eq!(next_leaf_index, 1);

        Ok(())
    }

    #[tokio::test]
    async fn mark_all_as_pending_marks_all() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);
        let roots = mock_roots(5);

        let mut pre_root = &initial_root;
        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
        }

        let mut tx = db.begin().await?;
        tx.mark_root_as_processed(&roots[2]).await?;
        tx.commit().await?;

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

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);
        let roots = mock_roots(5);

        let mut pre_root = &initial_root;
        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
        }

        let mut tx = db.begin().await?;
        tx.mark_root_as_processed(&roots[2]).await?;
        tx.commit().await?;

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

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);
        let roots = mock_roots(5);

        let mut pre_root = &initial_root;
        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
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

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(num_identities);
        let roots = mock_roots(num_identities);

        let mut pre_root = &initial_root;
        for i in 0..num_identities {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
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

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);
        let roots = mock_roots(5);

        let mut pre_root = &initial_root;
        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
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

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);
        let roots = mock_roots(5);

        let root = db.get_root_state(&roots[0]).await?;

        assert!(root.is_none(), "Root should not exist");

        db.insert_pending_identity(
            0,
            &identities[0],
            Some(Utc::now()),
            &roots[0],
            &initial_root,
        )
        .await?;
        let time_of_insertion = Utc::now();

        let root = db
            .get_root_state(&roots[0])
            .await?
            .context("Fetching root state")?;

        assert_same_time!(root.pending_valid_as_of, time_of_insertion);

        assert!(
            root.mined_valid_as_of.is_none(),
            "Root has not yet been mined"
        );

        db.mark_root_as_processed(&roots[0]).await?;
        let time_of_mining = Utc::now();

        let root = db
            .get_root_state(&roots[0])
            .await?
            .context("Fetching root state")?;

        let mined_valid_as_of = root
            .mined_valid_as_of
            .context("Root should have been mined")?;

        assert_same_time!(root.pending_valid_as_of, time_of_insertion);
        assert_same_time!(mined_valid_as_of, time_of_mining);

        Ok(())
    }

    #[tokio::test]
    async fn get_commitments_by_status() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);

        let roots = mock_roots(7);

        let mut pre_root = &initial_root;
        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
        }

        db.mark_root_as_processed(&roots[2]).await?;

        let mined_tree_updates = db
            .get_tree_updates_by_status(ProcessedStatus::Processed)
            .await?;
        let pending_tree_updates = db
            .get_tree_updates_by_status(ProcessedStatus::Pending)
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

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);

        let roots = mock_roots(5);
        let zero_roots = mock_zero_roots(5);

        let mut pre_root = &initial_root;
        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
        }

        db.insert_pending_identity(0, &Hash::ZERO, Some(Utc::now()), &zero_roots[0], &roots[4])
            .await?;
        db.insert_pending_identity(
            3,
            &Hash::ZERO,
            Some(Utc::now()),
            &zero_roots[3],
            &zero_roots[0],
        )
        .await?;

        let pending_tree_updates = db
            .get_tree_updates_by_status(ProcessedStatus::Pending)
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
    async fn root_invalidation() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);
        let roots = mock_roots(5);

        db.insert_pending_identity(
            0,
            &identities[0],
            Some(Utc::now()),
            &roots[0],
            &initial_root,
        )
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
        db.insert_pending_identity(1, &identities[1], Some(Utc::now()), &roots[1], &roots[0])
            .await?;
        db.insert_pending_identity(2, &identities[2], Some(Utc::now()), &roots[2], &roots[1])
            .await?;

        let root_1_inserted_at = Utc::now();

        tokio::time::sleep(Duration::from_secs(2)).await; // sleep enough for the database time resolution

        let root_item_0 = db.get_root_state(&roots[0]).await?.unwrap();
        let root_item_1 = db.get_root_state(&roots[1]).await?.unwrap();

        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);
        assert_same_time!(root_item_1.pending_valid_as_of, root_1_inserted_at);

        // Test mined roots
        db.insert_pending_identity(3, &identities[3], Some(Utc::now()), &roots[3], &roots[2])
            .await?;

        db.mark_root_as_processed(&roots[0])
            .await
            .context("Marking root as mined")?;

        let root_0_mined_at = Utc::now();

        tokio::time::sleep(Duration::from_secs(2)).await; // sleep enough for the database time resolution

        let root_item_2 = db.get_root_state(&roots[2]).await?.unwrap();
        assert!(matches!(root_item_2.status, ProcessedStatus::Pending));
        assert!(root_item_2.mined_valid_as_of.is_none());

        let root_item_1 = db.get_root_state(&roots[1]).await?.unwrap();
        assert_eq!(root_item_1.status, ProcessedStatus::Pending);
        assert!(root_item_1.mined_valid_as_of.is_none());
        assert!(root_item_1.pending_valid_as_of < root_0_mined_at);

        let root_item_0 = db.get_root_state(&roots[0]).await?.unwrap();
        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);
        assert_eq!(root_item_0.status, ProcessedStatus::Processed);
        assert_same_time!(root_item_0.mined_valid_as_of.unwrap(), root_0_mined_at);
        assert!(root_item_0.mined_valid_as_of.unwrap() > root_1_inserted_at);
        assert!(root_item_0.pending_valid_as_of < root_1_inserted_at);

        Ok(())
    }

    #[tokio::test]
    async fn check_identity_existence() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(2);
        let roots = mock_roots(1);

        // When there's no identity
        assert!(!db.identity_exists(identities[0]).await?);

        db.insert_unprocessed_identity(identities[0])
            .await
            .context("Inserting new identity")?;
        assert!(db.identity_exists(identities[0]).await?);

        // When there's only processed identity
        db.insert_pending_identity(
            0,
            &identities[1],
            Some(Utc::now()),
            &roots[0],
            &initial_root,
        )
        .await
        .context("Inserting identity")?;

        assert!(db.identity_exists(identities[1]).await?);

        Ok(())
    }

    #[tokio::test]
    async fn remove_deletions() -> anyhow::Result<()> {
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
    async fn latest_insertion() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        // Update with initial timestamp
        let initial_timestamp = chrono::Utc::now();
        db.update_latest_insertion(initial_timestamp)
            .await
            .context("Inserting initial root")?;

        // Assert values
        let initial_entry = db.get_latest_insertion().await?;
        assert!(initial_entry.timestamp.timestamp() - initial_timestamp.timestamp() <= 1);

        // Update with a new timestamp
        let new_timestamp = chrono::Utc::now();
        db.update_latest_insertion(new_timestamp)
            .await
            .context("Updating with new root")?;

        // Assert values
        let new_entry = db.get_latest_insertion().await?;
        assert!((new_entry.timestamp.timestamp() - new_timestamp.timestamp()) <= 1);

        Ok(())
    }

    #[tokio::test]
    async fn latest_deletion() -> anyhow::Result<()> {
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
    async fn can_not_insert_same_root_multiple_times() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(2);
        let roots = mock_roots(2);

        db.insert_pending_identity(
            0,
            &identities[0],
            Some(Utc::now()),
            &roots[0],
            &initial_root,
        )
        .await?;

        let res = db
            .insert_pending_identity(1, &identities[1], Some(Utc::now()), &roots[0], &roots[0])
            .await;

        assert!(res.is_err(), "Inserting duplicate root should fail");

        Ok(())
    }

    #[tokio::test]
    async fn insert_batch() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities: Vec<_> = mock_identities(10)
            .iter()
            .map(|commitment| {
                Identity::new(
                    (*commitment).into(),
                    mock_roots(10).iter().map(|root| (*root).into()).collect(),
                )
            })
            .collect();
        let roots = mock_roots(2);

        db.insert_new_batch_head(&roots[0]).await?;
        db.insert_new_batch(
            &roots[1],
            &roots[0],
            BatchType::Insertion,
            &identities,
            &[0],
        )
        .await?;

        Ok(())
    }

    #[tokio::test]
    async fn get_next_batch() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities: Vec<_> = mock_identities(10)
            .iter()
            .map(|commitment| {
                Identity::new(
                    (*commitment).into(),
                    mock_roots(10).iter().map(|root| (*root).into()).collect(),
                )
            })
            .collect();
        let indexes = vec![0];
        let roots = mock_roots(2);

        db.insert_new_batch_head(&roots[0]).await?;
        db.insert_new_batch(
            &roots[1],
            &roots[0],
            BatchType::Insertion,
            &identities,
            &indexes,
        )
        .await?;

        let next_batch = db.get_next_batch(&roots[0]).await?;

        assert!(next_batch.is_some());

        let next_batch = next_batch.unwrap();

        assert_eq!(next_batch.prev_root.unwrap(), roots[0]);
        assert_eq!(next_batch.next_root, roots[1]);
        assert_eq!(next_batch.data.0.identities, identities);
        assert_eq!(next_batch.data.0.indexes, indexes);

        let next_batch = db.get_next_batch(&roots[1]).await?;

        assert!(next_batch.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn get_next_batch_without_transaction() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let identities: Vec<_> = mock_identities(10)
            .iter()
            .map(|commitment| {
                Identity::new(
                    (*commitment).into(),
                    mock_roots(10).iter().map(|root| (*root).into()).collect(),
                )
            })
            .collect();
        let indexes = vec![0];
        let roots = mock_roots(2);
        let transaction_id = String::from("173bcbfd-e1d9-40e2-ba10-fc1dfbf742c9");

        db.insert_new_batch_head(&roots[0]).await?;
        db.insert_new_batch(
            &roots[1],
            &roots[0],
            BatchType::Insertion,
            &identities,
            &indexes,
        )
        .await?;

        let next_batch = db.get_next_batch_without_transaction().await?;

        assert!(next_batch.is_some());

        let next_batch = next_batch.unwrap();

        assert_eq!(next_batch.prev_root.unwrap(), roots[0]);
        assert_eq!(next_batch.next_root, roots[1]);
        assert_eq!(next_batch.data.0.identities, identities);
        assert_eq!(next_batch.data.0.indexes, indexes);

        db.insert_new_transaction(&transaction_id, &roots[1])
            .await?;

        let next_batch = db.get_next_batch_without_transaction().await?;

        assert!(next_batch.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn get_batch_head() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let roots = mock_roots(1);

        let batch_head = db.get_batch_head().await?;

        assert!(batch_head.is_none());

        db.insert_new_batch_head(&roots[0]).await?;

        let batch_head = db.get_batch_head().await?;

        assert!(batch_head.is_some());
        let batch_head = batch_head.unwrap();

        assert_eq!(batch_head.prev_root, None);
        assert_eq!(batch_head.next_root, roots[0]);
        assert!(
            batch_head.data.0.identities.is_empty(),
            "Should have empty identities."
        );
        assert!(
            batch_head.data.0.indexes.is_empty(),
            "Should have empty indexes."
        );

        Ok(())
    }

    #[tokio::test]
    async fn insert_transaction() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let roots = mock_roots(1);
        let transaction_id = String::from("173bcbfd-e1d9-40e2-ba10-fc1dfbf742c9");

        db.insert_new_batch_head(&roots[0]).await?;

        db.insert_new_transaction(&transaction_id, &roots[0])
            .await?;

        Ok(())
    }

    #[tokio::test]
    async fn get_tree_item_by_statuses_single_status() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);
        let roots = mock_roots(5);

        // Insert identities with pending status
        let mut pre_root = &initial_root;
        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
        }

        // Mark first 3 as processed
        db.mark_root_as_processed(&roots[2]).await?;

        // Test getting by Processed status
        let item = db
            .get_tree_item_by_statuses(&identities[0], vec![ProcessedStatus::Processed])
            .await?;
        assert!(item.is_some(), "Should find processed identity");
        let item = item.unwrap();
        assert_eq!(item.element, identities[0]);
        assert_eq!(item.leaf_index, 0);
        assert_eq!(item.status, ProcessedStatus::Processed);

        // Test getting by Pending status
        let item = db
            .get_tree_item_by_statuses(&identities[3], vec![ProcessedStatus::Pending])
            .await?;
        assert!(item.is_some(), "Should find pending identity");
        let item = item.unwrap();
        assert_eq!(item.element, identities[3]);
        assert_eq!(item.leaf_index, 3);
        assert_eq!(item.status, ProcessedStatus::Pending);

        // Test querying processed identity with wrong status
        let item = db
            .get_tree_item_by_statuses(&identities[0], vec![ProcessedStatus::Pending])
            .await?;
        assert!(
            item.is_none(),
            "Should not find processed identity when querying for pending"
        );

        Ok(())
    }

    #[tokio::test]
    async fn get_tree_item_by_statuses_multiple_statuses() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);
        let roots = mock_roots(5);

        // Insert identities
        let mut pre_root = &initial_root;
        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
        }

        // Mark some as processed and some as mined
        db.mark_root_as_processed(&roots[2]).await?;
        db.mark_root_as_mined(&roots[1]).await?;

        // Test getting mined identity with multiple statuses
        let item = db
            .get_tree_item_by_statuses(
                &identities[0],
                vec![ProcessedStatus::Pending, ProcessedStatus::Mined],
            )
            .await?;
        assert!(
            item.is_some(),
            "Should find mined identity when querying for pending or mined"
        );
        let item = item.unwrap();
        assert_eq!(item.element, identities[0]);
        assert_eq!(item.status, ProcessedStatus::Mined);

        // Test getting processed identity with multiple statuses
        let item = db
            .get_tree_item_by_statuses(
                &identities[2],
                vec![ProcessedStatus::Processed, ProcessedStatus::Mined],
            )
            .await?;
        assert!(
            item.is_some(),
            "Should find processed identity when querying for processed or mined"
        );
        let item = item.unwrap();
        assert_eq!(item.element, identities[2]);
        assert_eq!(item.status, ProcessedStatus::Processed);

        // Test getting pending identity with all statuses
        let item = db
            .get_tree_item_by_statuses(
                &identities[4],
                vec![
                    ProcessedStatus::Pending,
                    ProcessedStatus::Processed,
                    ProcessedStatus::Mined,
                ],
            )
            .await?;
        assert!(
            item.is_some(),
            "Should find pending identity when querying for all statuses"
        );
        let item = item.unwrap();
        assert_eq!(item.element, identities[4]);
        assert_eq!(item.status, ProcessedStatus::Pending);

        Ok(())
    }

    #[tokio::test]
    async fn get_tree_item_by_statuses_nonexistent_identity() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let nonexistent_identity = Field::from(999999);

        // Test with single status
        let item = db
            .get_tree_item_by_statuses(&nonexistent_identity, vec![ProcessedStatus::Pending])
            .await?;
        assert!(
            item.is_none(),
            "Should return None for nonexistent identity"
        );

        // Test with multiple statuses
        let item = db
            .get_tree_item_by_statuses(
                &nonexistent_identity,
                vec![
                    ProcessedStatus::Pending,
                    ProcessedStatus::Processed,
                    ProcessedStatus::Mined,
                ],
            )
            .await?;
        assert!(
            item.is_none(),
            "Should return None for nonexistent identity with multiple statuses"
        );

        Ok(())
    }

    #[tokio::test]
    async fn get_tree_item_by_statuses_returns_latest() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(5);
        let roots = mock_roots(5);

        // Insert multiple identities
        let mut pre_root = &initial_root;
        for i in 0..5 {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
        }

        // Mark first 2 as processed
        db.mark_root_as_processed(&roots[1]).await?;

        // Mark first 1 as mined
        db.mark_root_as_mined(&roots[0]).await?;

        // Query for identity[0] - it has transitioned from Pending -> Processed -> Mined
        // The latest status should be Mined
        let item = db
            .get_tree_item_by_statuses(&identities[0], vec![ProcessedStatus::Mined])
            .await?;
        assert!(
            item.is_some(),
            "Should find the identity with latest status (Mined)"
        );
        let item = item.unwrap();
        assert_eq!(item.element, identities[0]);
        assert_eq!(item.leaf_index, 0);
        assert_eq!(item.status, ProcessedStatus::Mined);

        // Query for identity[1] - it should be Processed
        let item = db
            .get_tree_item_by_statuses(&identities[1], vec![ProcessedStatus::Processed])
            .await?;
        assert!(item.is_some(), "Should find processed identity");
        let item = item.unwrap();
        assert_eq!(item.element, identities[1]);
        assert_eq!(item.status, ProcessedStatus::Processed);

        // When querying with multiple statuses, should still return the item with current status
        let item = db
            .get_tree_item_by_statuses(
                &identities[0],
                vec![
                    ProcessedStatus::Pending,
                    ProcessedStatus::Processed,
                    ProcessedStatus::Mined,
                ],
            )
            .await?;
        assert!(item.is_some());
        let item = item.unwrap();
        assert_eq!(item.status, ProcessedStatus::Mined);

        Ok(())
    }

    #[tokio::test]
    async fn get_tree_item_by_statuses_status_transitions() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;

        let initial_root = LazyPoseidonTree::new(4, Hash::ZERO).root();
        let identities = mock_identities(3);
        let roots = mock_roots(3);

        // Insert identities
        let mut pre_root = &initial_root;
        for i in 0..3 {
            db.insert_pending_identity(i, &identities[i], Some(Utc::now()), &roots[i], pre_root)
                .await
                .context("Inserting identity")?;
            pre_root = &roots[i];
        }

        // Initially all are pending
        let item = db
            .get_tree_item_by_statuses(&identities[0], vec![ProcessedStatus::Pending])
            .await?;
        assert!(item.is_some());
        assert_eq!(item.unwrap().status, ProcessedStatus::Pending);

        // Mark as processed
        db.mark_root_as_processed(&roots[0]).await?;

        // Should not be found as pending anymore
        let item = db
            .get_tree_item_by_statuses(&identities[0], vec![ProcessedStatus::Pending])
            .await?;
        assert!(
            item.is_none(),
            "Should not find as pending after processing"
        );

        // Should be found as processed
        let item = db
            .get_tree_item_by_statuses(&identities[0], vec![ProcessedStatus::Processed])
            .await?;
        assert!(item.is_some());
        assert_eq!(item.unwrap().status, ProcessedStatus::Processed);

        // Mark as mined
        db.mark_root_as_mined(&roots[0]).await?;

        // Should not be found as processed anymore
        let item = db
            .get_tree_item_by_statuses(&identities[0], vec![ProcessedStatus::Processed])
            .await?;
        assert!(item.is_none(), "Should not find as processed after mining");

        // Should be found as mined
        let item = db
            .get_tree_item_by_statuses(&identities[0], vec![ProcessedStatus::Mined])
            .await?;
        assert!(item.is_some());
        assert_eq!(item.unwrap().status, ProcessedStatus::Mined);

        // Should be found when querying for mined OR processed
        let item = db
            .get_tree_item_by_statuses(
                &identities[0],
                vec![ProcessedStatus::Processed, ProcessedStatus::Mined],
            )
            .await?;
        assert!(item.is_some());
        assert_eq!(item.unwrap().status, ProcessedStatus::Mined);

        Ok(())
    }
}
