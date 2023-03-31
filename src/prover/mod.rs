pub mod batch_insertion;
pub mod proof;

use std::{collections::BTreeMap, str::FromStr};

use clap::Parser;
use serde::{Deserialize, Serialize};

use self::batch_insertion::Prover;

#[derive(Clone, Debug, PartialEq, Eq, Parser)]
#[group(skip)]
pub struct Options {
    /// The options for configuring the batch insertion prover service.
    ///
    /// This should be a JSON array containing objects of the following format `{"url": "http://localhost:3001","batch_size": 3,"timeout_s": 30}`
    #[clap(
        long,
        env,
        default_value = r#"[{"url": "http://localhost:3001","batch_size": 3,"timeout_s": 30}]"#
    )]
    pub prover_urls: ProverOptionsWrapper,
}

/// A map that contains a prover for each batch size.
///
/// Provides utility methods for getting the appropriate provers
///
/// The struct is generic over P for testing purposes.
#[derive(Debug, Clone)]
pub struct ProverMap<P = Prover>(pub BTreeMap<usize, P>);

impl ProverMap {
    pub fn new(options: &Options) -> anyhow::Result<Self> {
        let mut map = BTreeMap::new();

        for url in &options.prover_urls.0 {
            map.insert(url.batch_size, Prover::new(url)?);
        }

        Ok(Self(map))
    }
}

impl<P> ProverMap<P> {
    /// Get the smallest prover that can handle the given batch size.
    pub fn get(&self, batch_size: usize) -> Option<&P> {
        self.0.iter().find_map(|(size, prover)| {
            if batch_size <= *size {
                Some(prover)
            } else {
                None
            }
        })
    }

    pub fn max_batch_size(&self) -> usize {
        self.0.iter().next_back().map_or(0, |(size, _)| *size)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProverOptionsWrapper(pub Vec<batch_insertion::Options>);

impl FromStr for ProverOptionsWrapper {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map(ProverOptionsWrapper)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prover_map_tests() {
        let prover_map: ProverMap<usize> = ProverMap(maplit::btreemap! {
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
