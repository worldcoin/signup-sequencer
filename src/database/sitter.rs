use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::TransactionReceipt;
use thiserror::Error;

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
        .try_into().expect("It's 2038, this code has been running for 16 years!")
}

impl super::Database {
    pub async fn insert_transaction_reqest(
        &self,
        id: &[u8],
        tx: TypedTransaction,
    ) -> Result<(), InsertTxError> {
        let received_at = secs_since_epoch();
        let serialized = serde_json::to_string(&tx)?;

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

    pub async fn unsubmitted_transaction_request(&self) -> Result<Option<(&[u8], TypedTransaction)>, anyhow::Error> {
        unimplemented!()
    }

    pub async fn submit_transaction(&self, id: &[u8], receipt: &TransactionReceipt) -> Result<(), anyhow::Error> {
        // insert a row into submitted_eth_tx
        unimplemented!()
    }
}

impl From<sqlx::Error> for InsertTxError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::Database(ref dberror) = err {
            let msg = dberror.message();
            // TODO: confirm that this also works for postgres; probably by running
            //       all our tests twice, once against each database.
            //       consider attempting to downcast into SqliteError and PostgresError
            //       in sqlite this is code 2067 and the full message is
            //       "UNIQUE constraint failed: tx_requests.idempotency_key",
            //       in postgres it will be 23505, unique_violation
            if msg.contains("UNIQUE") && msg.contains("id") {
                Self::DuplicateTransactionId
            } else {
                Self::Internal(err)
            }
        } else {
            Self::Internal(err)
        }
    }
}
