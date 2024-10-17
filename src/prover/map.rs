use std::collections::HashSet;

use crate::prover::{Prover, ProverConfig, ProverType};
use crate::utils::min_map::MinMap;

/// A map that contains a prover for each batch size.
///
/// Provides utility methods for getting the appropriate provers
#[derive(Debug, Default)]
pub struct ProverMap {
    map: MinMap<usize, Prover>,
}

impl ProverMap {
    /// Get the smallest prover that can handle the given batch size.
    pub fn get(&self, batch_size: usize) -> Option<&Prover> {
        self.map.get(batch_size)
    }

    /// Registers the provided `prover` for the given `batch_size` in the map.
    pub fn add(&mut self, batch_size: usize, prover: Prover) {
        self.map.add(batch_size, prover);
    }

    /// Removes the prover for the provided `batch_size` from the prover map.
    pub fn remove(&mut self, batch_size: usize) -> Option<Prover> {
        self.map.remove(batch_size)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn max_batch_size(&self) -> usize {
        self.map.max_key().unwrap_or(0)
    }

    pub fn batch_size_exists(&self, batch_size: usize) -> bool {
        self.map.key_exists(batch_size)
    }

    pub fn as_configuration_vec(&self) -> Vec<ProverConfig> {
        self.map
            .iter()
            .map(|(k, v)| ProverConfig {
                url: v.url(),
                timeout_s: v.timeout_s(),
                batch_size: *k,
                prover_type: v.prover_type(),
            })
            .collect()
    }
}

/// Builds an insertion prover map from the provided configuration.
pub fn initialize_prover_maps(
    db_provers: HashSet<ProverConfig>,
) -> anyhow::Result<(ProverMap, ProverMap)> {
    let mut insertion_map = ProverMap::default();
    let mut deletion_map = ProverMap::default();

    for prover in db_provers {
        match prover.prover_type {
            ProverType::Insertion => {
                insertion_map.add(prover.batch_size, Prover::from_prover_conf(&prover)?);
            }

            ProverType::Deletion => {
                deletion_map.add(prover.batch_size, Prover::from_prover_conf(&prover)?);
            }
        }
    }

    Ok((insertion_map, deletion_map))
}
