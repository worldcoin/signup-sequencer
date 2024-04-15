use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{Mutex, Notify};
use tracing::info;

use crate::database::types::DeletionEntry;
use crate::database::Database;
use crate::identity_tree::{Hash, Latest, TreeVersion};

pub struct DeleteIdentities {
    database:                 Arc<Database>,
    latest_tree:              TreeVersion<Latest>,
    deletion_time_interval:   i64,
    min_deletion_batch_size:  usize,
    wake_up_notify:           Arc<Notify>,
    pending_insertions_mutex: Arc<Mutex<()>>,
}

impl DeleteIdentities {
    pub fn new(
        database: Arc<Database>,
        latest_tree: TreeVersion<Latest>,
        deletion_time_interval: i64,
        min_deletion_batch_size: usize,
        wake_up_notify: Arc<Notify>,
        pending_insertions_mutex: Arc<Mutex<()>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            latest_tree,
            deletion_time_interval,
            min_deletion_batch_size,
            wake_up_notify,
            pending_insertions_mutex,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        delete_identities(
            &self.database,
            &self.latest_tree,
            self.deletion_time_interval,
            self.min_deletion_batch_size,
            self.wake_up_notify.clone(),
            &self.pending_insertions_mutex,
        )
        .await
    }
}

async fn delete_identities(
    database: &Database,
    latest_tree: &TreeVersion<Latest>,
    deletion_time_interval: i64,
    min_deletion_batch_size: usize,
    wake_up_notify: Arc<Notify>,
    pending_insertions_mutex: &Mutex<()>,
) -> anyhow::Result<()> {
    info!("Starting deletion processor.");

    let deletion_time_interval = chrono::Duration::seconds(deletion_time_interval);

    loop {
        let deletions = database.get_deletions().await?;

        // Early sleep if db is empty
        if deletions.is_empty() {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            continue;
        }

        let last_deletion_timestamp = database.get_latest_deletion().await?.timestamp;
        let time_since_last_deletion = Utc::now() - last_deletion_timestamp;
        let deletion_timed_out = time_since_last_deletion > deletion_time_interval;

        let deletions: HashSet<DeletionEntry> = if deletions.len() >= min_deletion_batch_size {
            take_multiples_of_deduped(deletions, min_deletion_batch_size)
        } else if deletion_timed_out {
            deletions.into_iter().collect()
        } else {
            HashSet::new()
        };

        // Sleep if not enough items for batches
        if deletions.is_empty() {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            continue;
        }

        let (leaf_indices, previous_commitments): (Vec<usize>, Vec<Hash>) = deletions
            .iter()
            .map(|d| (d.leaf_index, d.commitment))
            .unzip();

        let _guard = pending_insertions_mutex.lock().await;

        // Delete the commitments at the target leaf indices in the latest tree,
        // generating the proof for each update
        let data = latest_tree.delete_many(&leaf_indices);

        assert_eq!(
            data.len(),
            leaf_indices.len(),
            "Length mismatch when appending identities to tree"
        );

        // Insert the new items into pending identities
        let items = data.into_iter().zip(leaf_indices);
        for ((root, _proof), leaf_index) in items {
            database
                .insert_pending_identity(leaf_index, &Hash::ZERO, &root)
                .await?;
        }

        // Remove the previous commitments from the deletions table
        database.remove_deletions(previous_commitments).await?;
        wake_up_notify.notify_one();
    }
}

/// Takes n*multiple_of items from the input vector, deduplicating them and
/// preserving the order.
fn take_multiples_of_deduped<T>(items: Vec<T>, multiple_of: usize) -> HashSet<T>
where
    T: std::hash::Hash + Eq + Clone,
{
    let mut hashset = HashSet::new();
    let mut deduped = Vec::new();

    for item in items {
        if !hashset.contains(&item) {
            // I don't like cloning here but don't really see another way
            hashset.insert(item.clone());
            deduped.push(item);
        }
    }

    let num = (deduped.len() / multiple_of) * multiple_of;

    deduped.into_iter().take(num).collect()
}

#[cfg(test)]
mod tests {
    use maplit::hashset;
    use test_case::test_case;

    use super::*;

    #[test_case(vec![] => hashset! {})]
    #[test_case(vec![1] => hashset! {})]
    #[test_case(vec![1, 2] => hashset! {})]
    #[test_case(vec![1, 2, 3] => hashset! {1, 2, 3})]
    #[test_case(vec![1, 1, 1, 1] => hashset! {})]
    #[test_case(vec![1, 1, 2, 3] => hashset! {1, 2, 3})]
    #[test_case(vec![1, 1, 2, 3, 4, 5, 6, 7, 8] => hashset! {1, 2, 3, 4, 5, 6})]
    #[test_case(vec![1, 1, 2, 3, 4, 5, 6, 7, 8, 9, 1, 1, 1, 1, 1] => hashset! {1, 2, 3, 4, 5, 6, 7, 8, 9})]
    fn take_multiples_of_3(items: Vec<i32>) -> HashSet<i32> {
        take_multiples_of_deduped(items, 3)
    }
}
