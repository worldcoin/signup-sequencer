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
use sqlx::{Executor, Pool, Postgres, Row};
use thiserror::Error;
use tracing::{error, info, instrument, warn};

use crate::config::DatabaseConfig;
use crate::database::query::DatabaseQuery;
use crate::identity_tree::Hash;

pub mod query;
pub mod transaction;
pub mod types;

// Statically link in migration files
static MIGRATOR: Migrator = sqlx::migrate!("schemas/database");

pub struct Database {
    pub pool: Pool<Postgres>,
}

impl Deref for Database {
    type Target = Pool<Postgres>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

impl<'a, T> DatabaseQuery<'a> for T where T: Executor<'a, Database = Postgres> {}

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
    use crate::database::query::DatabaseQuery;
    use crate::database::types::BatchType;
    use crate::identity_tree::{Hash, ProcessedStatus, UnprocessedStatus};
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

        db.mark_root_as_processed_tx(&roots[2]).await?;

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

        db.mark_root_as_processed_tx(&roots[2]).await?;

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

        db.mark_root_as_mined_tx(&roots[2]).await?;

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
        db.mark_root_as_processed_tx(&roots[2]).await?;

        assert_roots_are(&db, &roots[..3], ProcessedStatus::Processed).await?;
        assert_roots_are(&db, &roots[3..], ProcessedStatus::Pending).await?;

        println!("Marking roots up to 1st as mined");
        db.mark_root_as_mined_tx(&roots[1]).await?;

        assert_roots_are(&db, &roots[..2], ProcessedStatus::Mined).await?;
        assert_roots_are(&db, &[roots[2]], ProcessedStatus::Processed).await?;
        assert_roots_are(&db, &roots[3..], ProcessedStatus::Pending).await?;

        println!("Marking roots up to 4th as processed");
        db.mark_root_as_processed_tx(&roots[4]).await?;

        assert_roots_are(&db, &roots[..2], ProcessedStatus::Mined).await?;
        assert_roots_are(&db, &roots[2..5], ProcessedStatus::Processed).await?;
        assert_roots_are(&db, &roots[5..], ProcessedStatus::Pending).await?;

        println!("Marking all roots as mined");
        db.mark_root_as_mined_tx(&roots[num_identities - 1]).await?;

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
        db.mark_root_as_processed_tx(&roots[2]).await?;

        // Later we correctly mark the previous root as mined
        db.mark_root_as_processed_tx(&roots[1]).await?;

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

        db.mark_root_as_processed_tx(&roots[0]).await?;

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

        db.mark_root_as_processed_tx(&roots[2]).await?;

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

        db.mark_root_as_processed_tx(&roots[0])
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
        db.insert_new_batch(&roots[1], &roots[0], BatchType::Insertion, &identities, &[
            0,
        ])
        .await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_get_next_batch() -> anyhow::Result<()> {
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
    async fn test_get_next_batch_without_transaction() -> anyhow::Result<()> {
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
    async fn test_get_batch_head() -> anyhow::Result<()> {
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
    async fn test_insert_transaction() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (db, _db_container) = setup_db(&docker).await?;
        let roots = mock_roots(1);
        let transaction_id = String::from("173bcbfd-e1d9-40e2-ba10-fc1dfbf742c9");

        db.insert_new_batch_head(&roots[0]).await?;

        db.insert_new_transaction(&transaction_id, &roots[0])
            .await?;

        Ok(())
    }
}
