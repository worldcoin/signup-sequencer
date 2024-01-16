use std::net::SocketAddr;
use std::time::Duration;

use semaphore::Field;
use serde::{Deserialize, Serialize};

use crate::prover::ProverConfig;
use crate::secret::SecretUrl;
use crate::serde_utils::JsonStrWrapper;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub app:      AppConfig,
    pub tree:     TreeConfig,
    pub database: DatabaseConfig,
    pub server:   ServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// A list of prover urls (along with batch size, type and timeout) that
    /// will be inserted into the DB at startup
    pub provers_urls: JsonStrWrapper<Vec<ProverConfig>>,

    /// The maximum number of seconds the sequencer will wait before sending a
    /// batch of identities to the chain, even if the batch is not full.
    #[serde(with = "humantime_serde")]
    #[serde(default = "default::batch_insertion_timeout")]
    pub batch_insertion_timeout: Duration,

    /// The maximum number of seconds the sequencer will wait before sending a
    /// batch of deletions to the chain, even if the batch is not full.
    #[serde(with = "humantime_serde")]
    #[serde(default = "default::batch_deletion_timeout")]
    pub batch_deletion_timeout: Duration,

    /// The smallest deletion batch size that we'll allow
    #[serde(default = "default::min_batch_deletion_size")]
    pub min_batch_deletion_size: usize,

    /// The parameter to control the delay between mining a deletion batch and
    /// inserting the recovery identities
    ///
    /// The sequencer will insert the recovery identities after
    /// max_epoch_duration_seconds + root_history_expiry) seconds have passed
    ///
    /// By default the value is set to 0 so the sequencer will only use
    /// root_history_expiry
    #[serde(with = "humantime_serde")]
    #[serde(default = "default::max_epoch_duration")]
    pub max_epoch_duration: Duration,

    /// The maximum number of windows to scan for finalization logs
    #[serde(default = "default::scanning_window_size")]
    pub scanning_window_size: u64,

    /// The offset from the latest block to scan
    #[serde(default = "default::scanning_chain_head_offset")]
    pub scanning_chain_head_offset: u64,

    /// The number of seconds to wait between fetching logs
    #[serde(with = "humantime_serde")]
    #[serde(default = "default::time_between_scans")]
    pub time_between_scans: Duration,

    /// The number of txs in the channel that we'll be monitoring
    #[serde(default = "default::monitored_txs_capacity")]
    pub monitored_txs_capacity: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeConfig {
    /// The depth of the tree that the contract is working with. This needs to
    /// agree with the verifier in the deployed contract, and also with
    /// `semaphore-mtb`
    #[serde(default = "default::tree_depth")]
    pub tree_depth: usize,

    /// The depth of the tree prefix that is vectorized
    #[serde(default = "default::dense_tree_prefix_depth")]
    pub dense_tree_prefix_depth: usize,

    /// The number of updates to trigger garbage collection
    #[serde(default = "default::tree_gc_threshold")]
    pub tree_gc_threshold: usize,

    /// Path and file name to use for mmap file when building dense tree
    #[serde(default)]
    pub cache_file: Option<String>,

    /// If set will not use cached tree state
    #[serde(default = "default::force_cache_purge")]
    pub force_cache_purge: bool,

    /// Initial value of the Merkle tree leaves. Defaults to the initial value
    /// used in the identity manager contract.
    #[serde(default = "default::initial_leaf_value")]
    pub initial_leaf_value: Field,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub database: SecretUrl,

    #[serde(default = "default::migrate")]
    pub migrate: bool,

    #[serde(default = "default::max_connections")]
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub address: SocketAddr,

    #[serde(with = "humantime_serde")]
    #[serde(default = "default::serve_timeout")]
    pub serve_timeout: Duration,
}

mod default {
    use std::time::Duration;

    pub fn batch_insertion_timeout() -> Duration {
        Duration::from_secs(180)
    }

    pub fn batch_deletion_timeout() -> Duration {
        Duration::from_secs(3600)
    }

    pub fn min_batch_deletion_size() -> usize {
        100
    }

    pub fn max_epoch_duration() -> Duration {
        Duration::from_secs(0)
    }

    pub fn scanning_window_size() -> u64 {
        100
    }

    pub fn scanning_chain_head_offset() -> u64 {
        0
    }

    pub fn time_between_scans() -> Duration {
        Duration::from_secs(30)
    }

    pub fn monitored_txs_capacity() -> usize {
        100
    }

    pub fn serve_timeout() -> Duration {
        Duration::from_secs(30)
    }

    pub fn migrate() -> bool {
        true
    }

    pub fn max_connections() -> u32 {
        10
    }

    pub fn tree_depth() -> usize {
        30
    }

    pub fn dense_tree_prefix_depth() -> usize {
        20
    }

    pub fn tree_gc_threshold() -> usize {
        10_000
    }

    pub fn force_cache_purge() -> bool {
        false
    }

    pub fn initial_leaf_value() -> semaphore::Field {
        semaphore::Field::from_be_bytes(hex_literal::hex!(
            "0000000000000000000000000000000000000000000000000000000000000000"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_TOML: &str = indoc::indoc! {r#"
        [app]
        provers_urls = "[]"

        [tree]

        [database]
        database = "postgres://user:password@localhost:5432/database"

        [server]
        address = "0.0.0.0:3001"
    "#};

    #[test]
    fn deserialize_minimal_config() {
        let _config: Config = toml::from_str(MINIMAL_TOML).unwrap();
    }

    const FULL_TOML: &str = indoc::indoc! {r#"
        [app]
        provers_urls = "[]"
        batch_insertion_timeout = "3m"
        batch_deletion_timeout = "1h"
        min_batch_deletion_size = 100
        max_epoch_duration = "0s"
        scanning_window_size = 100
        scanning_chain_head_offset = 0
        time_between_scans = "30s"
        monitored_txs_capacity = 100

        [tree]
        tree_depth = 30
        dense_tree_prefix_depth = 20
        tree_gc_threshold = 10000
        cache_file = "/data/cache_file"
        force_cache_purge = false
        initial_leaf_value = "0x0000000000000000000000000000000000000000000000000000000000000001"

        [database]
        database = "postgres://user:password@localhost:5432/database"
        migrate = true
        max_connections = 10

        [server]
        address = "0.0.0.0:3001"
        serve_timeout = "30s"
    "#};

    #[test]
    fn full_toml_round_trip() {
        let config: Config = toml::from_str(FULL_TOML).unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();

        similar_asserts::assert_eq!(serialized.trim(), FULL_TOML.trim());
    }
}
