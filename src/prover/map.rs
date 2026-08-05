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

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn max_batch_size(&self) -> usize {
        self.map.max_key().unwrap_or(0)
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
