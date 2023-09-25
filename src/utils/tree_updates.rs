use crate::identity_tree::TreeUpdate;

pub fn dedup_tree_updates(updates: Vec<TreeUpdate>) -> Vec<TreeUpdate> {
    let mut deduped = Vec::new();
    let mut temp: Option<TreeUpdate> = None;

    for update in updates {
        if let Some(prev) = temp {
            if prev.leaf_index == update.leaf_index {
                temp = Some(update);
            } else {
                deduped.push(prev);
                temp = Some(update);
            }
        } else {
            temp = Some(update);
        }
    }

    if let Some(item) = temp {
        deduped.push(item);
    }

    deduped
}

#[cfg(test)]
mod tests {
    use semaphore::Field;

    use super::*;
    use crate::identity_tree::Hash;

    #[test]
    fn deduplicates_tree_updates() {
        let hashes: Vec<Hash> = (0..10).map(Field::from).collect();

        let updates = vec![
            TreeUpdate::new(0, hashes[0]),
            TreeUpdate::new(1, hashes[1]),
            TreeUpdate::new(1, hashes[2]),
            TreeUpdate::new(1, hashes[3]),
            TreeUpdate::new(2, hashes[4]),
            TreeUpdate::new(2, hashes[5]),
            TreeUpdate::new(3, hashes[6]),
        ];
        let expected = vec![
            TreeUpdate::new(0, hashes[0]),
            TreeUpdate::new(1, hashes[3]),
            TreeUpdate::new(2, hashes[5]),
            TreeUpdate::new(3, hashes[6]),
        ];

        let deduped = dedup_tree_updates(updates);

        assert_eq!(expected, deduped);
    }
}
