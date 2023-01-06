use assert_matches::assert_matches;
use ethers::types::transaction::eip2718::TypedTransaction;
use rand::distributions::{Alphanumeric, DistString};
/// Tests which are run against both sqlite and postgres to ensure consistent
/// behavior.
///
/// The sqlite arm uses an in-memory database, but the postgres arm must be
/// provided a postgres database url via the `TEST_DATABASE` environment
/// variable.
use signup_sequencer::database::{self, Database};
use sqlx::{Connection, Executor};
use tracing::{error, info};
use tracing_subscriber::fmt::{format::FmtSpan, time::Uptime};
use url::Url;

#[derive(Debug)]
struct TempPgDb {
    server_url: String,
    dbname:     String,
    db_dropped: bool,

    pub db_url: String,
}

impl TempPgDb {
    pub async fn new(server_url: String) -> Self {
        let random_name = Alphanumeric.sample_string(&mut rand::thread_rng(), 10);
        let dbname = format!("tempdb_{}", random_name);

        let mut conn = sqlx::PgConnection::connect(&server_url)
            .await
            .expect("could not connect to database");

        conn.execute(format!(r#"CREATE DATABASE "{}""#, dbname).as_str())
            .await
            .expect("could not create test database");

        conn.close().await.expect("could not close connection");

        let db_url = format!("{}/{}", server_url, dbname);

        Self {
            server_url,
            dbname,
            db_dropped: false,
            db_url,
        }
    }

    pub async fn drop_database(mut self) {
        let mut conn = sqlx::PgConnection::connect(&self.server_url)
            .await
            .expect("could not connect to database");

        conn.execute(format!(r#"DROP DATABASE "{}""#, self.dbname).as_str())
            .await
            .expect("could not drop test database");

        self.db_dropped = true;
    }
}

impl Drop for TempPgDb {
    // it is inconvenient to do the actual dropping here because sqlx is async and
    // rust does not have a good async drop story
    fn drop(&mut self) {
        if !self.db_dropped {
            error!("TempPgDb was not dropped, a temporary database was left behind");
        }
    }
}

fn read_postgres_url() -> String {
    // TODO: do some validation
    std::env::var("TEST_DATABASE").expect("TEST_DATABASE must be set")
}

async fn test_database(url: &str) -> Database {
    let options = database::Options {
        database:                 Url::parse(url).expect("failed to parse TEST_DATABASE"),
        database_migrate:         true,
        database_max_connections: 10,
    };

    Database::new(options)
        .await
        .expect("failed to create database")
}

#[tokio::test]
async fn insert_duplicate_transaction_sqlite() {
    init_tracing_subscriber();

    let db = test_database("sqlite::memory:").await;
    inner_insert_duplicate_transaction(&db).await;
}

#[tokio::test]
async fn insert_duplicate_transaction_pg() {
    init_tracing_subscriber();

    let pg_url = read_postgres_url();
    let tempdb = TempPgDb::new(pg_url).await;

    {
        let db = test_database(&tempdb.db_url).await;
        inner_insert_duplicate_transaction(&db).await;

        db.close().await; // we cannot drop the db if there are active
                          // connections
    }

    tempdb.drop_database().await;
}

fn empty_transaction() -> TypedTransaction {
    TypedTransaction::Legacy(ethers::types::TransactionRequest::default())
}

async fn inner_insert_duplicate_transaction(db: &Database) {
    let id = [0u8; 32];

    db.insert_transaction_request(&id, &empty_transaction())
        .await
        .expect("could not insert first tx");

    let res = db
        .insert_transaction_request(&id, &empty_transaction())
        .await;

    assert_matches!(res, Err(database::InsertTxError::DuplicateTransactionId));
}

fn init_tracing_subscriber() {
    let result = tracing_subscriber::fmt()
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_line_number(true)
        .with_env_filter("info,signup_sequencer=debug")
        .with_timer(Uptime::default())
        .pretty()
        .try_init();
    if let Err(error) = result {
        error!(error, "Failed to initialize tracing_subscriber");
    }
}
