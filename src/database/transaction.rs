use sqlx::{Executor, Postgres, Transaction};
use tokio::sync::Mutex;
use tracing::instrument;

use crate::database::query::DatabaseQuery;
use crate::database::types::UnprocessedCommitment;
use crate::database::{Database, Error};
use crate::identity_tree::{Hash, Latest, ProcessedStatus, TreeVersion, TreeVersionReadOps};
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

pub async fn insert_identities_batch(
    tx: &mut Transaction<'_, Postgres>,
    latest_tree: &TreeVersion<Latest>,
    identities: &[UnprocessedCommitment],
    pending_insertions_mutex: &Mutex<()>,
) -> anyhow::Result<()> {
    // Filter out any identities that are already in the `identities` table
    let mut filtered_identities = vec![];
    for identity in identities {
        if tx
            .get_identity_leaf_index(&identity.commitment)
            .await?
            .is_some()
        {
            tracing::warn!(?identity.commitment, "Duplicate identity");
            tx.remove_unprocessed_identity(&identity.commitment).await?;
        } else {
            filtered_identities.push(identity.commitment);
        }
    }

    let _guard = pending_insertions_mutex.lock().await;

    let next_db_index = tx.get_next_leaf_index().await?;
    let next_leaf = latest_tree.next_leaf();

    assert_eq!(
        next_leaf, next_db_index,
        "Database and tree are out of sync. Next leaf index in tree is: {next_leaf}, in database: \
         {next_db_index}"
    );

    let data = latest_tree.append_many(&filtered_identities);

    assert_eq!(
        data.len(),
        filtered_identities.len(),
        "Length mismatch when appending identities to tree"
    );

    let items = data.into_iter().zip(filtered_identities);

    for ((root, _proof, leaf_index), identity) in items {
        tx.insert_pending_identity(leaf_index, &identity, &root)
            .await?;

        tx.remove_unprocessed_identity(&identity).await?;
    }

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

    #[instrument(level = "info", skip_all)]
    pub async fn insert_identities_batch_tx(
        &self,
        latest_tree: &TreeVersion<Latest>,
        identities: Vec<UnprocessedCommitment>,
        pending_insertions_mutex: &Mutex<()>,
    ) -> anyhow::Result<()> {
        retry_tx!(
            self.pool,
            tx,
            insert_identities_batch(&mut tx, latest_tree, &identities, pending_insertions_mutex)
                .await
        )
        .await
    }
}
