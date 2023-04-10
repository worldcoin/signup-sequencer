use std::{collections::BTreeMap, sync::Arc};

use crate::prover::batch_insertion;

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// A prover that can have read-only operations performed on it.
pub type ReadOnlyProver<'a, P> = RwLockReadGuard<'a, P>;

/// A prover that can have read and write operations performed on it.
pub type ReadWriteProver<'a, P> = RwLockWriteGuard<'a, P>;

/// The type of a map containing a mapping from a usize to a locked item.
type LockedMap<P> = BTreeMap<usize, RwLock<P>>;

/// A map that contains a prover for each batch size.
///
/// Provides utility methods for getting the appropriate provers
///
/// The struct is generic over P for testing purposes.
#[derive(Debug)]
pub struct ProverMap<P> {
    map: BTreeMap<usize, RwLock<P>>,
}

impl<P> ProverMap<P> {
    /// Get the smallest prover that can handle the given batch size.
    pub async fn get(&self, batch_size: usize) -> Option<ReadOnlyProver<P>> {
        for (size, prover) in self.map.iter() {
            if batch_size <= *size {
                return Some(prover.read().await);
            }
        }

        None
    }

    /// Removes the prover for the provided `batch_size` from the prover map.
    pub async fn remove(&mut self, batch_size: usize) -> Option<P> {
        // self.map.remove(&batch_size).map(|lock| lock.into_inner())
        unimplemented!()
    }

    pub fn max_batch_size(&self) -> usize {
        self.map.iter().next_back().map_or(0, |(size, _)| *size)
    }

    pub fn batch_size_exists(&self, batch_size: usize) -> bool {
        self.map.contains_key(&batch_size)
    }
}

/// A map of provers for batch insertion operations.
pub type InsertionProverMap = ProverMap<batch_insertion::Prover>;

impl InsertionProverMap {
    /// Constructs a new prover map containing the specified batch insertion
    /// provers.
    pub fn new(options: &batch_insertion::Options) -> anyhow::Result<Self> {
        let mut map = BTreeMap::new();

        for url in &options.prover_urls.0 {
            map.insert(
                url.batch_size,
                RwLock::new(batch_insertion::Prover::new(url)?),
            );
        }

        Ok(Self { map })
    }
}

impl<P> From<BTreeMap<usize, P>> for ProverMap<P> {
    fn from(input_map: BTreeMap<usize, P>) -> Self {
        let map: LockedMap<P> = input_map
            .into_iter()
            .map(|(batch_size, prover)| (batch_size, RwLock::new(prover)))
            .collect();

        Self { map }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prover_map_tests() {
        let prover_map: ProverMap<usize> = ProverMap::from(maplit::btreemap! {
            3 => 3,
            5 => 5,
            7 => 7,
        });

        assert_eq!(prover_map.max_batch_size(), 7);

        assert_eq!(*prover_map.get(1).await.unwrap(), 3);
        assert_eq!(*prover_map.get(2).await.unwrap(), 3);
        assert_eq!(*prover_map.get(3).await.unwrap(), 3);
        assert_eq!(*prover_map.get(4).await.unwrap(), 5);
        assert_eq!(*prover_map.get(7).await.unwrap(), 7);
        assert!(prover_map.get(8).await.is_none());
    }
}
