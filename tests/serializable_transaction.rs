mod common;
use common::prelude::*;
use futures::stream::StreamExt;
use signup_sequencer::retry_tx;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Postgres, Transaction};
use tokio::time::{sleep, Duration};

async fn setup(pool: &sqlx::Pool<Postgres>) -> Result<(), sqlx::Error> {
    retry_tx!(pool, tx, {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS accounts (
                id SERIAL PRIMARY KEY,
                balance INT
            );
            "#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query("TRUNCATE TABLE accounts RESTART IDENTITY;")
            .execute(&mut *tx)
            .await?;

        sqlx::query("INSERT INTO accounts (balance) VALUES (100), (200);")
            .execute(&mut *tx)
            .await?;

        Result::<_, anyhow::Error>::Ok(())
    })
    .await
    .unwrap();

    Ok(())
}

async fn transaction_1(pool: &sqlx::Pool<Postgres>) -> Result<(), sqlx::Error> {
    retry_tx!(pool, tx, {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
            .execute(&mut *tx)
            .await?;

        let balance: (i32,) = sqlx::query_as("SELECT balance FROM accounts WHERE id = 1")
            .fetch_one(&mut *tx)
            .await?;

        println!("Transaction 1: Balance of account 1 is {}", balance.0);

        // Simulate some work
        sleep(Duration::from_secs(5)).await;

        sqlx::query("UPDATE accounts SET balance = balance + 30 WHERE id = 2")
            .execute(&mut *tx)
            .await?;

        Result::<_, anyhow::Error>::Ok(())
    })
    .await
    .unwrap();

    Ok(())
}

async fn transaction_2(pool: &sqlx::Pool<Postgres>) -> Result<(), sqlx::Error> {
    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *tx)
        .await?;

    let balance: (i32,) = sqlx::query_as("SELECT balance FROM accounts WHERE id = 2")
        .fetch_one(&mut *tx)
        .await?;

    println!("Transaction 2: Balance of account 2 is {}", balance.0);

    sqlx::query("UPDATE accounts SET balance = balance + 50 WHERE id = 1")
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn serializable_transaction() -> Result<(), anyhow::Error> {
    init_tracing_subscriber();
    info!("Starting serializable_transaction");

    let insertion_batch_size: usize = 500;
    let deletion_batch_size: usize = 10;

    let ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let docker = Cli::default();
    let (mock_chain, db_container, _insertion_prover_map, _deletion_prover_map, micro_oz) =
        spawn_deps(
            initial_root,
            &[insertion_batch_size],
            &[deletion_batch_size],
            DEFAULT_TREE_DEPTH as u8,
            &docker,
        )
        .await?;

    let db_socket_addr = db_container.address();
    let db_url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

    let temp_dir = tempfile::tempdir()?;
    info!(
        "temp dir created at: {:?}",
        temp_dir.path().join("testfile")
    );

    let config = TestConfigBuilder::new()
        .db_url(&db_url)
        .oz_api_url(&micro_oz.endpoint())
        .oz_address(micro_oz.address())
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .build()?;

    let (..) = spawn_app(config.clone())
        .await
        .expect("Failed to spawn app.");

    let pool = PgPoolOptions::new()
        .max_connections(100)
        .connect(&db_url)
        .await?;

    setup(&pool).await?;
    futures::stream::iter(0..2)
        .for_each_concurrent(None, |_| async {
            transaction_1(&pool).await.unwrap();
            transaction_2(&pool).await.unwrap();
        })
        .await;

    Ok(())
}
