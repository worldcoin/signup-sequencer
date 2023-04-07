use std::collections::BTreeMap;

use crate::prover::batch_insertion;

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// A prover that can have read-only operations performed on it.
pub type ReadOnlyProver<'a, P> = RwLockReadGuard<'a, P>;

/// A prover that can have read and write operations performed on it.
pub type ReadWriteProver<'a, P> = RwLockWriteGuard<'a, P>;

/// A map that contains a prover for each batch size.
///
/// Provides utility methods for getting the appropriate provers
///
/// The struct is generic over P for testing purposes.
#[derive(Debug)]
pub struct ProverMap<P = batch_insertion::Prover> {
    map: BTreeMap<usize, P>,
}

// TODO [Ara] Map from usize to RWLock'd P, allowing fine-grained access.

impl ProverMap {
    pub fn new(options: &batch_insertion::Options) -> anyhow::Result<Self> {
        let mut map = BTreeMap::new();

        for url in &options.prover_urls.0 {
            map.insert(url.batch_size, batch_insertion::Prover::new(url)?);
        }

        Ok(Self { map })
    }
}

impl<P> ProverMap<P> {
    /// Get the smallest prover that can handle the given batch size.
    pub fn get(&self, batch_size: usize) -> Option<&P> {
        self.map
            .iter()
            .find_map(|(size, prover)| (batch_size <= *size).then_some(prover))
    }

    pub fn max_batch_size(&self) -> usize {
        self.map.iter().next_back().map_or(0, |(size, _)| *size)
    }
}

impl<P> From<BTreeMap<usize, P>> for ProverMap<P> {
    fn from(map: BTreeMap<usize, P>) -> Self {
        Self { map }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prover_map_tests() {
        let prover_map: ProverMap<usize> = ProverMap::from(maplit::btreemap! {
            3 => 3,
            5 => 5,
            7 => 7,
        });

        assert_eq!(prover_map.max_batch_size(), 7);

        assert_eq!(prover_map.get(1), Some(&3));
        assert_eq!(prover_map.get(2), Some(&3));
        assert_eq!(prover_map.get(3), Some(&3));
        assert_eq!(prover_map.get(4), Some(&5));
        assert_eq!(prover_map.get(7), Some(&7));
        assert_eq!(prover_map.get(8), None);
    }
}
