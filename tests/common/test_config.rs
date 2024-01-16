use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context;
use ethers::types::{Address, H160};
use signup_sequencer::config::{
    default, AppConfig, Config, DatabaseConfig, NetworkConfig, OzDefenderConfig, ProvidersConfig,
    RelayerConfig, ServerConfig, TreeConfig,
};
use signup_sequencer::prover::{ProverConfig, ProverType};
use signup_sequencer::secret::SecretUrl;
use url::Url;

use crate::ProverService;

pub struct TestConfigBuilder {
    tree_depth:               usize,
    prover_urls:              Vec<ProverConfig>,
    batch_insertion_timeout:  Duration,
    batch_deletion_timeout:   Duration,
    min_batch_deletion_size:  usize,
    db_url:                   Option<String>,
    oz_api_url:               Option<String>,
    oz_address:               Option<Address>,
    cache_file:               Option<String>,
    identity_manager_address: Option<Address>,
    primary_network_provider: Option<SecretUrl>,
}

impl TestConfigBuilder {
    pub fn new() -> Self {
        Self {
            tree_depth:               18,
            prover_urls:              vec![],
            batch_insertion_timeout:  Duration::from_secs(10),
            batch_deletion_timeout:   Duration::from_secs(10),
            min_batch_deletion_size:  1,
            db_url:                   None,
            oz_api_url:               None,
            oz_address:               None,
            cache_file:               None,
            identity_manager_address: None,
            primary_network_provider: None,
        }
    }

    pub fn db_url(mut self, db_url: &str) -> Self {
        self.db_url = Some(db_url.to_string());
        self
    }

    pub fn oz_api_url(mut self, oz_api_url: &str) -> Self {
        self.oz_api_url = Some(oz_api_url.to_string());
        self
    }

    pub fn oz_address(mut self, oz_address: Address) -> Self {
        self.oz_address = Some(oz_address);
        self
    }

    pub fn cache_file(mut self, cache_file: &str) -> Self {
        self.cache_file = Some(cache_file.to_string());
        self
    }

    pub fn identity_manager_address(mut self, identity_manager_address: Address) -> Self {
        self.identity_manager_address = Some(identity_manager_address);
        self
    }

    pub fn primary_network_provider(mut self, primary_network_provider: &SecretUrl) -> Self {
        self.primary_network_provider = Some(primary_network_provider.clone());
        self
    }

    pub fn add_prover(mut self, prover: &ProverService) -> Self {
        let prover_config = ProverConfig {
            url:         prover.url().to_string(),
            // TODO: Make this configurable?
            timeout_s:   30,
            batch_size:  prover.batch_size(),
            prover_type: prover.prover_type(),
        };

        self.prover_urls.push(prover_config);

        self
    }

    pub fn into_config(self) -> anyhow::Result<Config> {
        let db_url = self.db_url.context("Missing database url")?;
        let database = SecretUrl::new(Url::parse(&db_url)?);

        let config = Config {
            app:       AppConfig {
                provers_urls:               self.prover_urls.into(),
                batch_insertion_timeout:    self.batch_insertion_timeout,
                batch_deletion_timeout:     self.batch_deletion_timeout,
                min_batch_deletion_size:    self.min_batch_deletion_size,
                max_epoch_duration:         default::max_epoch_duration(),
                scanning_window_size:       default::scanning_window_size(),
                scanning_chain_head_offset: default::scanning_chain_head_offset(),
                time_between_scans:         default::time_between_scans(),
                monitored_txs_capacity:     default::monitored_txs_capacity(),
            },
            tree:      TreeConfig {
                tree_depth:              self.tree_depth,
                dense_tree_prefix_depth: default::dense_tree_prefix_depth(),
                tree_gc_threshold:       default::tree_gc_threshold(),
                cache_file:              self.cache_file.context("Missing cache file")?,
                force_cache_purge:       default::force_cache_purge(),
                initial_leaf_value:      default::initial_leaf_value(),
            },
            network:   NetworkConfig {
                identity_manager_address:           self
                    .identity_manager_address
                    .context("Missing identity manager address")?,
                relayed_identity_manager_addresses: Default::default(),
            },
            providers: ProvidersConfig {
                primary_network_provider:  self
                    .primary_network_provider
                    .context("Missing primary network provider")?,
                relayed_network_providers: Default::default(),
            },
            relayer:   RelayerConfig::OzDefender(OzDefenderConfig {
                oz_api_url:              self.oz_api_url.context("Missing oz api url")?,
                oz_address:              self.oz_address.context("Missing oz address")?,
                oz_api_key:              "".to_string(),
                oz_api_secret:           "".to_string(),
                oz_transaction_validity: default::oz_transaction_validity(),
                oz_send_timeout:         default::oz_send_timeout(),
                oz_mine_timeout:         default::oz_mine_timeout(),
                oz_gas_limit:            Default::default(),
            }),
            database:  DatabaseConfig {
                database,
                migrate: default::migrate(),
                max_connections: default::max_connections(),
            },
            server:    ServerConfig {
                address:       SocketAddr::from(([127, 0, 0, 1], 0)),
                serve_timeout: default::serve_timeout(),
            },
        };

        Ok(config)
    }
}
