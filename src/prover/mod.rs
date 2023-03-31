pub mod batch_insertion;
pub mod proof;

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

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
    pub prover_urls: JsonWrapper,
}

#[derive(Debug, Clone)]
pub struct ProverMap(pub BTreeMap<usize, Prover>);

impl ProverMap {
    pub fn new(options: &Options) -> anyhow::Result<Self> {
        let mut map = BTreeMap::new();

        for url in options.prover_urls.0.iter() {
            map.insert(url.batch_size, Prover::new(url)?);
        }

        Ok(Self(map))
    }

    /// Get the smallest prover that can handle the given batch size.
    pub fn get(&self, batch_size: usize) -> Option<&Prover> {
        self.0.iter().find_map(|(size, prover)| {
            if batch_size <= *size {
                Some(prover)
            } else {
                None
            }
        })
    }

    pub fn max_batch_size(&self) -> usize {
        self.0
            .iter()
            .next_back()
            .map(|(size, _)| *size)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonWrapper(pub Vec<batch_insertion::Options>);

impl FromStr for JsonWrapper {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map(JsonWrapper)
    }
}
