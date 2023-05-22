use std::{collections::HashSet, sync::Arc};

use anyhow::Result as AnyhowResult;
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use tracing::{error, instrument, warn};

use crate::{
    database::Database,
    identity_tree::{Hash, InclusionProof, Latest, Status, TreeVersion, TreeVersionReadOps},
};

pub enum OnInsertComplete {
    DuplicateCommitment,
    Proof(InclusionProof),
}

pub struct IdentityInsert {
    pub identity:    Hash,
    pub on_complete: oneshot::Sender<OnInsertComplete>,
}

pub struct InsertIdentities {
    database:          Arc<Database>,
    latest_tree:       TreeVersion<Latest>,
    identity_receiver: Arc<Mutex<mpsc::Receiver<IdentityInsert>>>,
    wake_up_notify:    Arc<Notify>,
}

impl InsertIdentities {
    pub fn new(
        database: Arc<Database>,
        latest_tree: TreeVersion<Latest>,
        identity_receiver: Arc<Mutex<mpsc::Receiver<IdentityInsert>>>,
        wake_up_notify: Arc<Notify>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            latest_tree,
            identity_receiver,
            wake_up_notify,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        let mut identity_receiver = self.identity_receiver.lock().await;

        insert_identities(
            &self.database,
            &self.latest_tree,
            &mut identity_receiver,
            &self.wake_up_notify,
        )
        .await
    }
}

#[instrument(level = "info", skip_all)]
async fn insert_identities(
    database: &Database,
    latest_tree: &TreeVersion<Latest>,
    identity_receiver: &mut mpsc::Receiver<IdentityInsert>,
    wake_up_notify: &Notify,
) -> AnyhowResult<()> {
    loop {
        // get commits from database
        let unprocessed = database.get_unprocessed_commitments("NEW").await?;

        // Dedup
        let mut commitments_set = HashSet::new();
        let mut deduped = Vec::with_capacity(unprocessed.len());

        for identity in unprocessed {
            if commitments_set.contains(&identity.commitment) {
                database
                    .update_err_unprocessed_commitment(
                        identity.commitment,
                        "Duplicate commitment.".into(),
                    )
                    .await?;
            } else {
                commitments_set.insert(identity.commitment);
                deduped.push(identity);
            }
        }

        // Validate the identities are not in the database
        let mut identities = Vec::with_capacity(deduped.len());
        for identity in deduped {
            if database
                .get_identity_leaf_index(&identity.commitment)
                .await?
                .is_some()
            {
                database
                    .update_err_unprocessed_commitment(
                        identity.commitment,
                        "Duplicate commitment.".into(),
                    )
                    .await?;
            } else {
                identities.push(identity);
            }
        }

        let next_db_index = database.get_next_leaf_index().await?;
        let next_leaf = latest_tree.next_leaf();

        assert_eq!(
            next_leaf, next_db_index,
            "Database and tree are out of sync. Next leaf index in tree is: {next_leaf}, in \
             database: {next_db_index}"
        );

        let identities: Vec<Hash> = identities
            .into_iter()
            .map(|insert| insert.commitment)
            .collect();

        let data = latest_tree.append_many(&identities);

        assert_eq!(
            data.len(),
            identities.len(),
            "Length mismatch when appending identities to tree"
        );

        let items = data.into_iter().zip(identities.into_iter());

        for ((root, proof, leaf_index), identity) in items {
            database
                .insert_pending_identity(leaf_index, &identity, &root)
                .await?;

            // TODO: update db, set status of unprocessed to something?
        }

        // Notify the identity processing task, that there are new identities
        wake_up_notify.notify_one();
    }

    Ok(())
}

fn three_way_zip<A, B, C>(
    a: impl Iterator<Item = A>,
    b: impl Iterator<Item = B>,
    c: impl Iterator<Item = C>,
) -> impl Iterator<Item = (A, B, C)> {
    a.zip(b).zip(c).map(|((a, b), c)| (a, b, c))
}
