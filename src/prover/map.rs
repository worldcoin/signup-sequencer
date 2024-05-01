use std::collections::BTreeMap;

use tokio::sync::{RwLock, RwLockReadGuard};

use crate::prover::{Prover, ProverConfiguration, ProverType, Provers};

/// The type of a map containing a mapping from a usize to a locked item.
type SharedProverMap<P> = RwLock<ProverMap<P>>;

/// A prover that can have read-only operations performed on it.
pub type ReadOnlyProver<'a, P> = RwLockReadGuard<'a, P>;

/// A map that contains a prover for each batch size.
///
/// Provides utility methods for getting the appropriate provers
///
/// The struct is generic over P for testing purposes.
#[derive(Debug)]
pub struct ProverMap<P> {
    map: BTreeMap<usize, P>,
}

impl<P> ProverMap<P> {
    pub fn new(map: BTreeMap<usize, P>) -> Self {
        Self { map }
    }

    /// Get the smallest prover that can handle the given batch size.
    pub fn get(&self, batch_size: usize) -> Option<&P> {
        for (size, prover) in &self.map {
            if batch_size <= *size {
                return Some(prover);
            }
        }

        None
    }

    /// Registers the provided `prover` for the given `batch_size` in the map.
    pub fn add(&mut self, batch_size: usize, prover: P) {
        self.map.insert(batch_size, prover);
    }

    /// Removes the prover for the provided `batch_size` from the prover map.
    pub fn remove(&mut self, batch_size: usize) -> Option<P> {
        self.map.remove(&batch_size)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn max_batch_size(&self) -> usize {
        self.map.iter().next_back().map_or(0, |(size, _)| *size)
    }

    pub fn batch_size_exists(&self, batch_size: usize) -> bool {
        self.map.contains_key(&batch_size)
    }
}

impl ProverMap<Prover> {
    pub fn as_configuration_vec(&self) -> Vec<ProverConfiguration> {
        self.map
            .iter()
            .map(|(k, v)| ProverConfiguration {
                url:         v.url(),
                timeout_s:   v.timeout_s(),
                batch_size:  *k,
                prover_type: v.prover_type(),
            })
            .collect()
    }

    pub async fn max_available_batch_size(&self) -> anyhow::Result<usize> {
        // Iterate through the provers, starting from the largest batch size and check
        // if they are available.
        for (batch_size, prover) in self.map.iter().rev() {
            if let Ok(_) = prover.health_check().await {
                return Ok(*batch_size);
            }
        }

        anyhow::bail!("No available prover")
    }
}

impl<P> From<BTreeMap<usize, P>> for ProverMap<P> {
    fn from(map: BTreeMap<usize, P>) -> Self {
        Self { map }
    }
}

/// A map of provers for batch insertion operations.
pub type InsertionProverMap = SharedProverMap<Prover>;
/// A map of provers for batch deletion operations.
pub type DeletionProverMap = SharedProverMap<Prover>;

/// The type of provers that can only be read from for insertion operations.
pub type ReadOnlyInsertionProver<'a> = ReadOnlyProver<'a, Prover>;

/// Builds an insertion prover map from the provided configuration.
pub fn initialize_prover_maps(
    db_provers: Provers,
) -> anyhow::Result<(InsertionProverMap, DeletionProverMap)> {
    let mut insertion_map = BTreeMap::new();
    let mut deletion_map = BTreeMap::new();

    for prover in db_provers {
        match prover.prover_type {
            ProverType::Insertion => {
                insertion_map.insert(prover.batch_size, Prover::from_prover_conf(&prover)?);
            }

            ProverType::Deletion => {
                deletion_map.insert(prover.batch_size, Prover::from_prover_conf(&prover)?);
            }
        }
    }

    Ok((
        RwLock::new(ProverMap::new(insertion_map)),
        RwLock::new(ProverMap::new(deletion_map)),
    ))
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

        assert_eq!(prover_map.get(1), Some(&3));
        assert_eq!(prover_map.get(2), Some(&3));
        assert_eq!(prover_map.get(3), Some(&3));
        assert_eq!(prover_map.get(4), Some(&5));
        assert_eq!(prover_map.get(7), Some(&7));
        assert!(prover_map.get(8).is_none());
    }
}
