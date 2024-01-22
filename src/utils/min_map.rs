use std::collections::BTreeMap;

/// A wrapper over a BTreeMap that returns a given value if the key is at least
/// a value present in internal map
#[derive(Debug)]
pub struct MinMap<K, T> {
    map: BTreeMap<K, T>,
}

impl<K, T> Default for MinMap<K, T> {
    fn default() -> Self {
        Self {
            map: BTreeMap::default(),
        }
    }
}

impl<K, T> MinMap<K, T>
where
    K: Ord + Copy,
{
    pub fn new() -> Self {
        Self {
            map: BTreeMap::default(),
        }
    }

    /// Get the smallest value that is smaller than the given key
    pub fn get(&self, key: K) -> Option<&T> {
        for (size, value) in &self.map {
            if key <= *size {
                return Some(value);
            }
        }

        None
    }

    pub fn add(&mut self, key: K, value: T) {
        self.map.insert(key, value);
    }

    pub fn remove(&mut self, key: K) -> Option<T> {
        self.map.remove(&key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn max_key(&self) -> Option<K> {
        self.map.keys().next_back().copied()
    }

    pub fn key_exists(&self, key: K) -> bool {
        self.map.contains_key(&key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &T)> {
        self.map.iter()
    }
}

impl<K, T> From<BTreeMap<K, T>> for MinMap<K, T> {
    fn from(map: BTreeMap<K, T>) -> Self {
        Self { map }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn min_map_tests() {
        let min_map: MinMap<usize, usize> = MinMap::from(maplit::btreemap! {
            3 => 3,
            5 => 5,
            7 => 7,
        });

        assert_eq!(min_map.max_key(), Some(7));

        assert_eq!(min_map.get(1), Some(&3));
        assert_eq!(min_map.get(2), Some(&3));
        assert_eq!(min_map.get(3), Some(&3));
        assert_eq!(min_map.get(4), Some(&5));
        assert_eq!(min_map.get(7), Some(&7));
        assert!(min_map.get(8).is_none());
    }
}
