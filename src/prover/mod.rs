pub mod batch_insertion;
pub mod proof;

use std::str::FromStr;

use clap::Parser;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonWrapper(pub Vec<batch_insertion::Options>);

impl FromStr for JsonWrapper {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map(JsonWrapper)
    }
}
