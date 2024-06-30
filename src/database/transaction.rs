use sqlx::{Executor, Postgres, Transaction};
use tracing::instrument;

use crate::database::query::DatabaseQuery;
use crate::database::{Database, Error};
use crate::identity_tree::{Hash, ProcessedStatus};
use crate::utils::retry_tx;

async fn mark_root_as_processed(
    tx: &mut Transaction<'_, Postgres>,
    root: &Hash,
) -> Result<(), Error> {
    let root_id = tx.get_id_by_root(root).await?;

    let Some(root_id) = root_id else {
        return Err(Error::MissingRoot { root: *root });
    };

    let root_id = root_id as i64;
    // TODO: Can I get rid of line `AND    status <> $2
    let update_previous_roots = sqlx::query(
        r#"
                UPDATE identities
                SET    status = $2, mined_at = CURRENT_TIMESTAMP
                WHERE  id <= $1
                AND    status <> $2
                AND    status <> $3;
                "#,
    )
    .bind(root_id)
    .bind(<&str>::from(ProcessedStatus::Processed))
    .bind(<&str>::from(ProcessedStatus::Mined));

    let update_next_roots = sqlx::query(
        r#"
                UPDATE identities
                SET    status = $2, mined_at = NULL
                WHERE  id > $1
                "#,
    )
    .bind(root_id)
    .bind(<&str>::from(ProcessedStatus::Pending));

    tx.execute(update_previous_roots).await?;
    tx.execute(update_next_roots).await?;

    Ok(())
}

pub async fn mark_root_as_mined(
    tx: &mut Transaction<'_, Postgres>,
    root: &Hash,
) -> Result<(), Error> {
    let mined_status = ProcessedStatus::Mined;

    let root_id = tx.get_id_by_root(root).await?;

    let Some(root_id) = root_id else {
        return Err(Error::MissingRoot { root: *root });
    };

    let root_id = root_id as i64;

    let update_previous_roots = sqlx::query(
        r#"
                UPDATE identities
                SET    status = $2
                WHERE  id <= $1
                AND    status <> $2
                "#,
    )
    .bind(root_id)
    .bind(<&str>::from(mined_status));

    tx.execute(update_previous_roots).await?;

    Ok(())
}

/// impl block for database transactions
impl Database {
    /// Marks the identities and roots from before a given root hash as mined
    /// Also marks following roots as pending
    #[instrument(skip(self), level = "debug")]
    pub async fn mark_root_as_processed_tx(&self, root: &Hash) -> Result<(), Error> {
        retry_tx!(self.pool, tx, mark_root_as_processed(&mut tx, root).await).await
    }

    /// Marks the identities and roots from before a given root hash as mined
    /// Also marks following roots as pending
    #[instrument(skip(self), level = "debug")]
    pub async fn mark_root_as_processed_and_delete_batches_tx(
        &self,
        root: &Hash,
    ) -> Result<(), Error> {
        retry_tx!(self.pool, tx, {
            mark_root_as_processed(&mut tx, root).await?;
            tx.delete_batches_after_root(root).await?;
            Ok(())
        })
        .await
    }

    /// Marks the identities and roots from before a given root hash as
    /// finalized
    #[instrument(skip(self), level = "debug")]
    pub async fn mark_root_as_mined_tx(&self, root: &Hash) -> Result<(), Error> {
        retry_tx!(self.pool, tx, mark_root_as_mined(&mut tx, root).await).await
    }
}
