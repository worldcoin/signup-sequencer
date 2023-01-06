use ethers::types::{transaction::eip2718::TypedTransaction, TransactionReceipt};
use sqlx::error::DatabaseError; // SqliteError::code()
use std::borrow::Cow;
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum InsertTxError {
    #[error("internal error: {0}")]
    Internal(sqlx::Error),

    #[error("a transaction with this id has already been received")]
    DuplicateTransactionId,

    #[error("cannot serialize transaction: {0}")]
    Serialize(#[from] serde_json::Error),
}

fn secs_since_epoch() -> i64 {
    // TODO: u64 is the correct type but it is not supported by sqlx::Any
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time is before UNIX_EPOCH")
        .as_secs()
        .try_into()
        .expect("It's 2038, this code has been running for 16 years!")
}

impl super::Database {
    pub async fn insert_transaction_request(
        &self,
        id: &[u8],
        tx: &TypedTransaction,
    ) -> Result<(), InsertTxError> {
        let received_at = secs_since_epoch();
        let serialized = serde_json::to_vec(&tx)?;

        sqlx::query::<sqlx::Any>(
            r#"
            INSERT INTO transaction_requests
                (id, received_at, serialized_tx )
            VALUES ($1, $2, $3);
            "#,
        )
        .bind(id)
        .bind(received_at)
        .bind(serialized)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn unsubmitted_transaction_request(
        &self,
    ) -> Result<Option<(&[u8], TypedTransaction)>, anyhow::Error> {
        unimplemented!()
    }

    pub async fn submit_transaction(
        &self,
        id: &[u8],
        receipt: &TransactionReceipt,
    ) -> Result<(), anyhow::Error> {
        // insert a row into submitted_eth_tx
        unimplemented!()
    }
}

impl From<sqlx::Error> for InsertTxError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::Database(ref dberror) = err {
            if let Some(pg_error) = dberror.try_downcast_ref::<sqlx::postgres::PgDatabaseError>() {
                // https://www.postgresql.org/docs/current/errcodes-appendix.html
                let unique_violation = "23505";
                if pg_error.code() == unique_violation {
                    return Self::DuplicateTransactionId;
                }
            }

            if let Some(sqlite_error) = dberror.try_downcast_ref::<sqlx::sqlite::SqliteError>() {
                // https://www.sqlite.org/rescode.html#constraint_primarykey
                let constraint_primarykey = "1555";
                let code: Option<Cow<'_, str>> = sqlite_error.code();

                if let Some(code) = code {
                    if code == constraint_primarykey {
                        return Self::DuplicateTransactionId;
                    }
                }
            }
        }

        Self::Internal(err)
    }
}
