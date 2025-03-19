use crate::identity_tree::TreeUpdate;

/// Deduplicates changes to same leaf. Requires as input updates sorted by leaf
/// index and also for same leaf index sorted in chronological order.
#[must_use]
pub fn dedup_tree_updates(updates: Vec<TreeUpdate>) -> Vec<TreeUpdate> {
    let mut deduped = Vec::new();
    let mut temp: Option<TreeUpdate> = None;

    for update in updates {
        if let Some(prev) = temp {
            if prev.leaf_index != update.leaf_index {
                deduped.push(prev);
            }
        }
        temp = Some(update);
    }

    if let Some(item) = temp {
        deduped.push(item);
    }

    deduped
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use semaphore_rs::Field;

    use super::*;

    fn new_tree_update(sequence_id: usize, leaf_index: usize) -> TreeUpdate {
        TreeUpdate::new(
            sequence_id,
            leaf_index,
            Field::from(sequence_id),
            Field::from(sequence_id),
            DateTime::from_timestamp_millis(1742309721 + (sequence_id as i64)),
        )
    }

    #[test]
    fn deduplicates_tree_updates() {
        let updates = vec![
            new_tree_update(1, 0),
            new_tree_update(2, 1),
            new_tree_update(3, 1),
            new_tree_update(4, 1),
            new_tree_update(5, 2),
            new_tree_update(6, 2),
            new_tree_update(7, 3),
        ];
        let expected = vec![
            new_tree_update(1, 0),
            new_tree_update(4, 1),
            new_tree_update(6, 2),
            new_tree_update(7, 3),
        ];

        let deduped = dedup_tree_updates(updates);

        assert_eq!(expected, deduped);
    }

    #[test]
    fn deduplicates_tree_updates_with_same_last() {
        let updates = vec![
            new_tree_update(1, 0),
            new_tree_update(2, 1),
            new_tree_update(3, 1),
            new_tree_update(4, 1),
            new_tree_update(5, 2),
            new_tree_update(6, 2),
            new_tree_update(7, 3),
            new_tree_update(8, 3),
        ];
        let expected = vec![
            new_tree_update(1, 0),
            new_tree_update(4, 1),
            new_tree_update(6, 2),
            new_tree_update(8, 3),
        ];

        let deduped = dedup_tree_updates(updates);

        assert_eq!(expected, deduped);
    }
}
