use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use ethers::types::{Address, H160};
use semaphore::Field;
use serde::{Deserialize, Serialize};
use telemetry_batteries::metrics::prometheus::PrometheusExporterConfig;

use crate::prover::ProverConfig;
use crate::utils::secret::SecretUrl;
use crate::utils::serde_utils::JsonStrWrapper;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub app:       AppConfig,
    pub tree:      TreeConfig,
    pub network:   NetworkConfig,
    pub providers: ProvidersConfig,
    pub relayer:   RelayerConfig,
    pub database:  DatabaseConfig,
    pub server:    ServerConfig,
    #[serde(default)]
    pub service:   ServiceConfig,
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

    // TODO: Allow running without a cache file
    /// Path and file name to use for mmap file when building dense tree
    #[serde(default = "default::cache_file")]
    pub cache_file: String,

    /// If set will not use cached tree state
    #[serde(default = "default::force_cache_purge")]
    pub force_cache_purge: bool,

    /// Initial value of the Merkle tree leaves. Defaults to the initial value
    /// used in the identity manager contract.
    #[serde(default = "default::initial_leaf_value")]
    pub initial_leaf_value: Field,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// The address of the identity manager contract.
    pub identity_manager_address: Address,

    /// The addresses of world id contracts on secondary chains
    /// mapped by chain id
    #[serde(default)]
    pub relayed_identity_manager_addresses: JsonStrWrapper<HashMap<u64, Address>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersConfig {
    /// Provider url for the primary chain
    pub primary_network_provider: SecretUrl,

    /// Provider urls for the secondary chains
    #[serde(default)]
    pub relayed_network_providers: JsonStrWrapper<Vec<SecretUrl>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
#[serde(rename_all = "snake_case")]
pub enum RelayerConfig {
    OzDefender(OzDefenderConfig),
    TxSitter(TxSitterConfig),
}

impl RelayerConfig {
    // TODO: Extract into a common field
    pub fn address(&self) -> Address {
        match self {
            RelayerConfig::OzDefender(config) => config.oz_address,
            RelayerConfig::TxSitter(config) => config.tx_sitter_address,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OzDefenderConfig {
    /// Api url
    #[serde(default = "default::oz_api_url")]
    pub oz_api_url: String,

    /// OpenZeppelin Defender API Key
    pub oz_api_key: String,

    /// OpenZeppelin Defender API Secret
    pub oz_api_secret: String,

    /// Address of OZ Relayer
    pub oz_address: H160,

    /// For how long should we track and retry the transaction (in
    /// seconds) Default: 7 days (7 * 24 * 60 * 60 = 604800 seconds)
    #[serde(with = "humantime_serde")]
    #[serde(default = "default::oz_transaction_validity")]
    pub oz_transaction_validity: Duration,

    #[serde(with = "humantime_serde")]
    #[serde(default = "default::oz_send_timeout")]
    pub oz_send_timeout: Duration,

    #[serde(with = "humantime_serde")]
    #[serde(default = "default::oz_mine_timeout")]
    pub oz_mine_timeout: Duration,

    pub oz_gas_limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxSitterConfig {
    pub tx_sitter_url: String,

    pub tx_sitter_address: H160,

    pub tx_sitter_gas_limit: Option<u64>,
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    // Service name - used for logging, metrics and tracing
    #[serde(default = "default::service_name")]
    pub service_name: String,
    pub datadog:      Option<DatadogConfig>,
    #[serde(default = "default::metrics")]
    pub metrics:      Option<MetricsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricsConfig {
    Prometheus(PrometheusExporterConfig),
    Statsd(StatsdConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatadogConfig {
    pub traces_endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsdConfig {
    pub metrics_host:        String,
    pub metrics_port:        u16,
    pub metrics_queue_size:  usize,
    pub metrics_buffer_size: usize,
    pub metrics_prefix:      String,
}

pub mod default {
    use std::time::Duration;

    use telemetry_batteries::metrics::prometheus::PrometheusExporterConfig;

    use super::MetricsConfig;

    pub fn service_name() -> String {
        "signup_sequencer".to_string()
    }

    pub fn metrics() -> Option<MetricsConfig> {
        Some(MetricsConfig::Prometheus(
            PrometheusExporterConfig::HttpListener {
                listen_address: "0.0.0.0:9998".parse().unwrap(),
            },
        ))
    }

    pub fn oz_api_url() -> String {
        "https://api.defender.openzeppelin.com".to_string()
    }

    pub fn oz_transaction_validity() -> Duration {
        Duration::from_secs(604800)
    }

    pub fn oz_send_timeout() -> Duration {
        Duration::from_secs(60)
    }

    pub fn oz_mine_timeout() -> Duration {
        Duration::from_secs(60)
    }

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

    pub fn cache_file() -> String {
        "/data/cache_file".to_string()
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

        [network]
        identity_manager_address = "0x0000000000000000000000000000000000000000"

        [providers]
        primary_network_provider = "http://localhost:8545"

        [relayer]
        kind = "tx_sitter"
        tx_sitter_url = "http://localhost:3000"
        tx_sitter_address = "0x0000000000000000000000000000000000000000"

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
        initial_leaf_value = "0x1"

        [network]
        identity_manager_address = "0x0000000000000000000000000000000000000000"
        relayed_identity_manager_addresses = "{}"

        [providers]
        primary_network_provider = "http://localhost:8545/"
        relayed_network_providers = "[]"

        [relayer]
        kind = "tx_sitter"
        tx_sitter_url = "http://localhost:3000"
        tx_sitter_address = "0x0000000000000000000000000000000000000000"
        tx_sitter_gas_limit = 100000

        [database]
        database = "postgres://user:password@localhost:5432/database"
        migrate = true
        max_connections = 10

        [server]
        address = "0.0.0.0:3001"
        serve_timeout = "30s"

        [service]
        service_name = "signup-sequencer"

        [service.datadog]
        traces_endpoint = "http://localhost:8126"

        [service.metrics.prometheus.http_listener]
        listen_address = "0.0.0.0:9998"
    "#};

    #[test]
    fn full_toml_round_trip() {
        let config: Config = toml::from_str(FULL_TOML).unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        similar_asserts::assert_eq!(serialized.trim(), FULL_TOML.trim());
    }
}
