use std::{collections::HashSet, sync::Arc};

use anyhow::Result as AnyhowResult;
use tokio::sync::{mpsc, oneshot, Mutex};
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
    wake_up_sender:    mpsc::Sender<()>,
}

impl InsertIdentities {
    pub fn new(
        database: Arc<Database>,
        latest_tree: TreeVersion<Latest>,
        identity_receiver: Arc<Mutex<mpsc::Receiver<IdentityInsert>>>,
        wake_up_sender: mpsc::Sender<()>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            latest_tree,
            identity_receiver,
            wake_up_sender,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        let mut identity_receiver = self.identity_receiver.lock().await;

        insert_identities(
            &self.database,
            &self.latest_tree,
            &mut identity_receiver,
            &self.wake_up_sender,
        )
        .await
    }
}

#[instrument(level = "info", skip_all)]
async fn insert_identities(
    database: &Database,
    latest_tree: &TreeVersion<Latest>,
    identity_receiver: &mut mpsc::Receiver<IdentityInsert>,
    wake_up_sender: &mpsc::Sender<()>,
) -> AnyhowResult<()> {
    loop {
        let Some(first_identity) = identity_receiver.recv().await else {
            warn!("Identity channel closed, terminating.");
            break;
        };

        // Get as many identities to commit in bulk
        let mut commitments = HashSet::new();
        commitments.insert(first_identity.identity);
        let mut identities = vec![first_identity];

        while let Ok(identity) = identity_receiver.try_recv() {
            if commitments.contains(&identity.identity)
                || database
                    .get_identity_leaf_index(&identity.identity)
                    .await?
                    .is_some()
            {
                identity
                    .on_complete
                    .send(OnInsertComplete::DuplicateCommitment)
                    .ok();
            } else {
                identities.push(identity);
            }
        }

        let next_db_index = database.get_next_leaf_index().await?;
        let next_leaf = latest_tree.next_leaf();

        assert!(
            next_leaf == next_db_index,
            "Database and tree are out of sync. Next leaf index in tree is: {}, in database: {}",
            next_leaf,
            next_db_index
        );

        let (identities, on_completes): (Vec<_>, Vec<_>) = identities
            .into_iter()
            .map(|insert| (insert.identity, insert.on_complete))
            .unzip();

        let data = latest_tree.append_many(&identities);

        assert_eq!(
            data.len(),
            identities.len(),
            "Length mismatch when appending identities to tree"
        );

        let items = three_way_zip(
            data.into_iter(),
            identities.into_iter(),
            on_completes.into_iter(),
        );

        for ((root, proof, leaf_index), identity, on_complete) in items {
            database
                .insert_pending_identity(leaf_index, &identity, &root)
                .await?;

            let inclusion_proof = InclusionProof {
                status: Status::Pending,
                root,
                proof,
            };

            if on_complete
                .send(OnInsertComplete::Proof(inclusion_proof))
                .is_err()
            {
                error!("On complete channel was closed before identity was inserted");
            }
        }

        // Notify the identity processing task, that there are new identities
        if wake_up_sender.send(()).await.is_err() {
            error!("Failed to wake up identity committer");
        }
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
