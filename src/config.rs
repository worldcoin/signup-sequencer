use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use ethers::types::{Address, H160};
use semaphore_rs::Field;
use serde::{Deserialize, Serialize};

use crate::prover::ProverConfig;
use crate::utils::secret::SecretUrl;
use crate::utils::serde_utils::JsonStrWrapper;

/// Authentication mode for the server API endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// No auth required (for testing/emergencies)
    #[default]
    Disabled,
    /// Basic Auth required only
    BasicOnly,
    /// Basic Auth required + soft-validate JWT (warn if missing, error if invalid)
    BasicWithSoftJwt,
    /// JWT required
    JwtOnly,
}

pub fn load_config(config_file_path: Option<&Path>) -> anyhow::Result<Config> {
    let mut settings = config::Config::builder();

    if let Some(path) = config_file_path {
        settings = settings.add_source(config::File::from(path).required(true));
    }

    let settings = settings
        .add_source(
            config::Environment::with_prefix("SEQ")
                .separator("__")
                .try_parsing(true),
        )
        .build()?;

    Ok(settings.try_deserialize::<Config>()?)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub app: AppConfig,
    pub tree: TreeConfig,
    #[serde(default)]
    pub network: Option<NetworkConfig>,
    #[serde(default)]
    pub providers: Option<ProvidersConfig>,
    #[serde(default)]
    pub relayer: Option<RelayerConfig>,
    pub database: DatabaseConfig,
    pub server: ServerConfig,
    #[serde(default)]
    pub service: ServiceConfig,
    #[serde(default)]
    pub offchain_mode: OffchainModeConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    /// The durtaion to wait for tasks to shutdown
    /// before timing out
    #[serde(with = "humantime_serde")]
    #[serde(default = "default::shutdown_timeout")]
    pub shutdown_timeout: Duration,

    /// The minimum amount of time to wait after a shutdown
    /// is innitiated before the process exits. This is useful to
    /// give cancelled tasks a chance to get to an await point.
    #[serde(with = "humantime_serde")]
    #[serde(default = "default::shutdown_delay")]
    pub shutdown_delay: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// The address of the identity manager contract.
    pub identity_manager_address: Address,

    /// The addresses of world id contracts on secondary chains
    /// mapped by chain id
    #[serde(default)]
    pub relayed_identity_manager_addresses: JsonStrWrapper<HashMap<u64, Address>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvidersConfig {
    /// Provider url for the primary chain
    pub primary_network_provider: SecretUrl,

    /// Provider urls for the secondary chains
    #[serde(default)]
    pub relayed_network_providers: JsonStrWrapper<Vec<SecretUrl>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxSitterConfig {
    pub tx_sitter_url: String,

    pub tx_sitter_address: H160,

    pub tx_sitter_gas_limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub database: SecretUrl,

    #[serde(default = "default::migrate")]
    pub migrate: bool,

    #[serde(default = "default::max_connections")]
    pub max_connections: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub address: SocketAddr,

    #[serde(with = "humantime_serde")]
    #[serde(default = "default::serve_timeout")]
    pub serve_timeout: Duration,

    /// Authentication mode
    #[serde(default)]
    pub auth_mode: AuthMode,

    /// Basic auth credentials (username -> password)
    #[serde(default)]
    pub basic_auth_credentials: HashMap<String, String>,

    /// Named authorized keys for JWT authentication: key_name -> PEM public key
    #[serde(default)]
    pub authorized_keys: HashMap<String, String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceConfig {
    // Service name - used for logging, metrics and tracing
    #[serde(default = "default::service_name")]
    pub service_name: String,
    pub datadog: Option<DatadogConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatadogConfig {
    pub traces_endpoint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffchainModeConfig {
    #[serde(default = "default::offchain_mode_enabled")]
    pub enabled: bool,
}

pub mod default {
    use std::time::Duration;

    pub fn service_name() -> String {
        "signup_sequencer".to_string()
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

    pub fn scanning_window_size() -> u64 {
        100
    }

    pub fn scanning_chain_head_offset() -> u64 {
        0
    }

    pub fn time_between_scans() -> Duration {
        Duration::from_secs(30)
    }

    pub fn shutdown_timeout() -> Duration {
        Duration::from_secs(30)
    }

    pub fn shutdown_delay() -> Duration {
        Duration::from_secs(1)
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

    pub fn initial_leaf_value() -> semaphore_rs::Field {
        semaphore_rs::Field::from_be_bytes(hex_literal::hex!(
            "0000000000000000000000000000000000000000000000000000000000000000"
        ))
    }

    pub fn offchain_mode_enabled() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const MINIMAL_TOML: &str = indoc::indoc! {r#"
        [app]
        provers_urls = "[]"

        [tree]

        [database]
        database = "postgres://user:password@localhost:5432/database"

        [server]
        address = "0.0.0.0:3001"

        [offchain_mode]
        enabled = false
    "#};

    const FULL_TOML: &str = indoc::indoc! {r#"
        [app]
        provers_urls = "[]"
        batch_insertion_timeout = "3m"
        batch_deletion_timeout = "1h"
        min_batch_deletion_size = 100
        scanning_window_size = 100
        scanning_chain_head_offset = 0
        time_between_scans = "30s"
        monitored_txs_capacity = 100
        shutdown_timeout = "30s"
        shutdown_delay = "1s"

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
        auth_mode = "disabled"

        [server.basic_auth_credentials]

        [server.authorized_keys]

        [service]
        service_name = "signup-sequencer"

        [service.datadog]
        traces_endpoint = "http://localhost:8126"

        [offchain_mode]
        enabled = false
    "#};

    const OFFCHAIN_TOML: &str = indoc::indoc! {r#"
        [app]
        provers_urls = "[]"
        batch_insertion_timeout = "3m"
        batch_deletion_timeout = "1h"
        min_batch_deletion_size = 100
        scanning_window_size = 100
        scanning_chain_head_offset = 0
        time_between_scans = "30s"
        monitored_txs_capacity = 100
        shutdown_timeout = "30s"
        shutdown_delay = "1s"

        [tree]
        tree_depth = 30
        dense_tree_prefix_depth = 20
        tree_gc_threshold = 10000
        cache_file = "/data/cache_file"
        force_cache_purge = false
        initial_leaf_value = "0x1"

        [database]
        database = "postgres://user:password@localhost:5432/database"
        migrate = true
        max_connections = 10

        [server]
        address = "0.0.0.0:3001"
        serve_timeout = "30s"
        auth_mode = "disabled"

        [server.basic_auth_credentials]

        [server.authorized_keys]

        [service]
        service_name = "signup-sequencer"

        [service.datadog]
        traces_endpoint = "http://localhost:8126"

        [offchain_mode]
        enabled = true
    "#};

    const FULL_ENV: &str = indoc::indoc! {r#"
        SEQ__APP__PROVERS_URLS=[]
        SEQ__APP__BATCH_INSERTION_TIMEOUT=3m
        SEQ__APP__BATCH_DELETION_TIMEOUT=1h
        SEQ__APP__MIN_BATCH_DELETION_SIZE=100
        SEQ__APP__SCANNING_WINDOW_SIZE=100
        SEQ__APP__SCANNING_CHAIN_HEAD_OFFSET=0
        SEQ__APP__TIME_BETWEEN_SCANS=30s
        SEQ__APP__MONITORED_TXS_CAPACITY=100
        SEQ__APP__SHUTDOWN_TIMEOUT=30s
        SEQ__APP__SHUTDOWN_DELAY=1s

        SEQ__TREE__TREE_DEPTH=30
        SEQ__TREE__DENSE_TREE_PREFIX_DEPTH=20
        SEQ__TREE__TREE_GC_THRESHOLD=10000
        SEQ__TREE__CACHE_FILE=/data/cache_file
        SEQ__TREE__FORCE_CACHE_PURGE=false
        SEQ__TREE__INITIAL_LEAF_VALUE=0x1

        SEQ__NETWORK__IDENTITY_MANAGER_ADDRESS=0x0000000000000000000000000000000000000000
        SEQ__NETWORK__RELAYED_IDENTITY_MANAGER_ADDRESSES={}

        SEQ__PROVIDERS__PRIMARY_NETWORK_PROVIDER=http://localhost:8545/
        SEQ__PROVIDERS__RELAYED_NETWORK_PROVIDERS=[]

        SEQ__RELAYER__KIND=tx_sitter
        SEQ__RELAYER__TX_SITTER_URL=http://localhost:3000
        SEQ__RELAYER__TX_SITTER_ADDRESS=0x0000000000000000000000000000000000000000
        SEQ__RELAYER__TX_SITTER_GAS_LIMIT=100000

        SEQ__DATABASE__DATABASE=postgres://user:password@localhost:5432/database
        SEQ__DATABASE__MIGRATE=true
        SEQ__DATABASE__MAX_CONNECTIONS=10

        SEQ__SERVER__ADDRESS=0.0.0.0:3001
        SEQ__SERVER__SERVE_TIMEOUT=30s
        SEQ__SERVER__AUTH_MODE=disabled

        SEQ__SERVICE__SERVICE_NAME=signup-sequencer

        SEQ__SERVICE__DATADOG__TRACES_ENDPOINT=http://localhost:8126

        SEQ__OFFCHAIN_MODE__ENABLED=false
    "#};

    const OFFCHAIN_ENV: &str = indoc::indoc! {r#"
        SEQ__APP__PROVERS_URLS=[]
        SEQ__APP__BATCH_INSERTION_TIMEOUT=3m
        SEQ__APP__BATCH_DELETION_TIMEOUT=1h
        SEQ__APP__MIN_BATCH_DELETION_SIZE=100
        SEQ__APP__SCANNING_WINDOW_SIZE=100
        SEQ__APP__SCANNING_CHAIN_HEAD_OFFSET=0
        SEQ__APP__TIME_BETWEEN_SCANS=30s
        SEQ__APP__MONITORED_TXS_CAPACITY=100
        SEQ__APP__SHUTDOWN_TIMEOUT=30s
        SEQ__APP__SHUTDOWN_DELAY=1s

        SEQ__TREE__TREE_DEPTH=30
        SEQ__TREE__DENSE_TREE_PREFIX_DEPTH=20
        SEQ__TREE__TREE_GC_THRESHOLD=10000
        SEQ__TREE__CACHE_FILE=/data/cache_file
        SEQ__TREE__FORCE_CACHE_PURGE=false
        SEQ__TREE__INITIAL_LEAF_VALUE=0x1

        SEQ__DATABASE__DATABASE=postgres://user:password@localhost:5432/database
        SEQ__DATABASE__MIGRATE=true
        SEQ__DATABASE__MAX_CONNECTIONS=10

        SEQ__SERVER__ADDRESS=0.0.0.0:3001
        SEQ__SERVER__SERVE_TIMEOUT=30s
        SEQ__SERVER__AUTH_MODE=disabled

        SEQ__SERVICE__SERVICE_NAME=signup-sequencer

        SEQ__SERVICE__DATADOG__TRACES_ENDPOINT=http://localhost:8126

        SEQ__OFFCHAIN_MODE__ENABLED=true
    "#};

    #[test]
    fn deserialize_minimal_config() {
        let _config: Config = toml::from_str(MINIMAL_TOML).unwrap();
    }

    #[test]
    fn full_toml_round_trip() {
        let config: Config = toml::from_str(FULL_TOML).unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        println!("{}", serialized);
        similar_asserts::assert_eq!(serialized.trim(), FULL_TOML.trim());
    }

    #[test]
    fn offchain_config() {
        let config: Config = toml::from_str(OFFCHAIN_TOML).unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        println!("{}", serialized);
        similar_asserts::assert_eq!(serialized.trim(), OFFCHAIN_TOML.trim());
    }

    // Necessary because the env tests might be run within the same process
    // so they would end up clashing on env var values
    lazy_static::lazy_static! {
        static ref ENV_MUTEX: Mutex<()> = Mutex::new(());
    }

    #[test]
    fn full_from_env() {
        let _lock = ENV_MUTEX.lock().unwrap();

        load_env(FULL_ENV);

        let parsed_config: Config = toml::from_str(FULL_TOML).unwrap();
        let env_config: Config = load_config(None).unwrap();

        assert_eq!(parsed_config, env_config);

        purge_env(FULL_ENV);
    }

    #[test]
    fn offchain_from_env() {
        let _lock = ENV_MUTEX.lock().unwrap();

        load_env(OFFCHAIN_ENV);

        let parsed_config: Config = toml::from_str(OFFCHAIN_TOML).unwrap();
        let env_config: Config = load_config(None).unwrap();

        assert_eq!(parsed_config, env_config);

        purge_env(OFFCHAIN_ENV);
    }

    fn load_env(s: &str) {
        for line in s.lines().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let mut parts = line.splitn(2, '=');
            let key = parts.next().expect("Missing key");
            let value = parts.next().expect("Missing value");

            println!("Setting '{}'='{}'", key, value);
            std::env::set_var(key, value);
        }
    }

    fn purge_env(s: &str) {
        for line in s.lines().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let mut parts = line.splitn(2, '=');
            let key = parts.next().expect("Missing key");

            std::env::remove_var(key);
        }
    }

    const AUTH_TOML: &str = indoc::indoc! {r#"
        [app]
        provers_urls = "[]"
        batch_insertion_timeout = "3m"
        batch_deletion_timeout = "1h"
        min_batch_deletion_size = 100
        scanning_window_size = 100
        scanning_chain_head_offset = 0
        time_between_scans = "30s"
        monitored_txs_capacity = 100
        shutdown_timeout = "30s"
        shutdown_delay = "1s"

        [tree]
        tree_depth = 30
        dense_tree_prefix_depth = 20
        tree_gc_threshold = 10000
        cache_file = "/data/cache_file"
        force_cache_purge = false
        initial_leaf_value = "0x1"

        [database]
        database = "postgres://user:password@localhost:5432/database"
        migrate = true
        max_connections = 10

        [server]
        address = "0.0.0.0:3001"
        serve_timeout = "30s"
        auth_mode = "basic_with_soft_jwt"

        [server.basic_auth_credentials]
        app_backend = "secretpass123"
        other_service = "otherpass456"

        [server.authorized_keys]
        app_backend = "test_public_key_pem_content"

        [service]
        service_name = "signup-sequencer"

        [service.datadog]
        traces_endpoint = "http://localhost:8126"

        [offchain_mode]
        enabled = true
    "#};

    const AUTH_ENV: &str = indoc::indoc! {r#"
        SEQ__APP__PROVERS_URLS=[]
        SEQ__APP__BATCH_INSERTION_TIMEOUT=3m
        SEQ__APP__BATCH_DELETION_TIMEOUT=1h
        SEQ__APP__MIN_BATCH_DELETION_SIZE=100
        SEQ__APP__SCANNING_WINDOW_SIZE=100
        SEQ__APP__SCANNING_CHAIN_HEAD_OFFSET=0
        SEQ__APP__TIME_BETWEEN_SCANS=30s
        SEQ__APP__MONITORED_TXS_CAPACITY=100
        SEQ__APP__SHUTDOWN_TIMEOUT=30s
        SEQ__APP__SHUTDOWN_DELAY=1s

        SEQ__TREE__TREE_DEPTH=30
        SEQ__TREE__DENSE_TREE_PREFIX_DEPTH=20
        SEQ__TREE__TREE_GC_THRESHOLD=10000
        SEQ__TREE__CACHE_FILE=/data/cache_file
        SEQ__TREE__FORCE_CACHE_PURGE=false
        SEQ__TREE__INITIAL_LEAF_VALUE=0x1

        SEQ__DATABASE__DATABASE=postgres://user:password@localhost:5432/database
        SEQ__DATABASE__MIGRATE=true
        SEQ__DATABASE__MAX_CONNECTIONS=10

        SEQ__SERVER__ADDRESS=0.0.0.0:3001
        SEQ__SERVER__SERVE_TIMEOUT=30s
        SEQ__SERVER__AUTH_MODE=basic_with_soft_jwt
        SEQ__SERVER__BASIC_AUTH_CREDENTIALS__APP_BACKEND=secretpass123
        SEQ__SERVER__BASIC_AUTH_CREDENTIALS__OTHER_SERVICE=otherpass456
        SEQ__SERVER__AUTHORIZED_KEYS__APP_BACKEND=test_public_key_pem_content

        SEQ__SERVICE__SERVICE_NAME=signup-sequencer

        SEQ__SERVICE__DATADOG__TRACES_ENDPOINT=http://localhost:8126

        SEQ__OFFCHAIN_MODE__ENABLED=true
    "#};

    #[test]
    fn auth_config_from_env() {
        let _lock = ENV_MUTEX.lock().unwrap();

        load_env(AUTH_ENV);

        let parsed_config: Config = toml::from_str(AUTH_TOML).unwrap();
        let env_config: Config = load_config(None).unwrap();

        // Verify auth mode
        assert_eq!(env_config.server.auth_mode, AuthMode::BasicWithSoftJwt);

        // Verify basic auth credentials
        assert_eq!(env_config.server.basic_auth_credentials.len(), 2);
        assert_eq!(
            env_config.server.basic_auth_credentials.get("app_backend"),
            Some(&"secretpass123".to_string())
        );
        assert_eq!(
            env_config
                .server
                .basic_auth_credentials
                .get("other_service"),
            Some(&"otherpass456".to_string())
        );

        // Verify authorized keys
        assert_eq!(env_config.server.authorized_keys.len(), 1);
        assert!(env_config
            .server
            .authorized_keys
            .contains_key("app_backend"));

        // Verify full config matches
        assert_eq!(parsed_config, env_config);

        purge_env(AUTH_ENV);
    }

    #[test]
    fn auth_mode_variants_from_env() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // Test all auth mode variants
        let modes = [
            ("disabled", AuthMode::Disabled),
            ("basic_only", AuthMode::BasicOnly),
            ("basic_with_soft_jwt", AuthMode::BasicWithSoftJwt),
            ("jwt_only", AuthMode::JwtOnly),
        ];

        for (env_value, expected_mode) in modes {
            // Load minimal env vars needed for config first
            load_env(OFFCHAIN_ENV);

            // Set auth_mode AFTER loading base env to override its default
            std::env::set_var("SEQ__SERVER__AUTH_MODE", env_value);

            let config: Config = load_config(None).unwrap();
            assert_eq!(
                config.server.auth_mode, expected_mode,
                "Failed for auth_mode={env_value}"
            );

            purge_env(OFFCHAIN_ENV);
            std::env::remove_var("SEQ__SERVER__AUTH_MODE");
        }
    }
}
