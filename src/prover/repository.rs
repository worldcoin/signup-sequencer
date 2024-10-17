use anyhow::anyhow;
use tokio::sync::{RwLock, RwLockReadGuard};
use tracing::warn;

use crate::prover::{Prover, ProverConfig, ProverMap, ProverType};

pub struct ProverRepository {
    insertion_prover_map: RwLock<ProverMap>,
    deletion_prover_map: RwLock<ProverMap>,
}

impl ProverRepository {
    pub fn new(insertion_prover_map: ProverMap, deletion_prover_map: ProverMap) -> Self {
        let insertion_prover_map = RwLock::new(insertion_prover_map);
        let deletion_prover_map = RwLock::new(deletion_prover_map);

        Self {
            insertion_prover_map,
            deletion_prover_map,
        }
    }

    pub async fn add_batch_size(
        &self,
        url: &impl ToString,
        batch_size: usize,
        timeout_seconds: u64,
        prover_type: ProverType,
    ) -> Result<(), crate::server::error::Error> {
        let mut map = match prover_type {
            ProverType::Insertion => self.insertion_prover_map.write().await,
            ProverType::Deletion => self.deletion_prover_map.write().await,
        };

        if map.batch_size_exists(batch_size) {
            return Err(crate::server::error::Error::BatchSizeAlreadyExists);
        }

        let prover = Prover::new(&ProverConfig {
            url: url.to_string(),
            batch_size,
            prover_type,
            timeout_s: timeout_seconds,
        })?;

        map.add(batch_size, prover);

        Ok(())
    }

    /// # Errors
    ///
    /// Will return `Err` if the batch size requested for removal doesn't exist
    /// in the prover map.
    pub async fn remove_batch_size(
        &self,
        batch_size: usize,
        prover_type: ProverType,
    ) -> Result<(), crate::server::error::Error> {
        let mut map = match prover_type {
            ProverType::Insertion => self.insertion_prover_map.write().await,
            ProverType::Deletion => self.deletion_prover_map.write().await,
        };

        if map.len() == 1 {
            warn!("Attempting to remove the last batch size.");
            return Err(crate::server::error::Error::CannotRemoveLastBatchSize);
        }

        match map.remove(batch_size) {
            Some(_) => Ok(()),
            None => Err(crate::server::error::Error::NoSuchBatchSize),
        }
    }

    pub async fn list_batch_sizes(&self) -> Result<Vec<ProverConfig>, crate::server::error::Error> {
        let mut provers = self
            .insertion_prover_map
            .read()
            .await
            .as_configuration_vec();

        provers.extend(self.deletion_prover_map.read().await.as_configuration_vec());

        Ok(provers)
    }

    pub async fn has_insertion_provers(&self) -> bool {
        self.insertion_prover_map.read().await.len() > 0
    }

    pub async fn has_deletion_provers(&self) -> bool {
        self.deletion_prover_map.read().await.len() > 0
    }

    pub async fn max_insertion_batch_size(&self) -> usize {
        self.insertion_prover_map.read().await.max_batch_size()
    }

    pub async fn max_deletion_batch_size(&self) -> usize {
        self.deletion_prover_map.read().await.max_batch_size()
    }

    pub async fn get_suitable_deletion_batch_size(
        &self,
        num_identities: usize,
    ) -> anyhow::Result<usize> {
        Ok(self
            .get_suitable_deletion_prover(num_identities)
            .await?
            .batch_size())
    }

    pub async fn get_suitable_insertion_batch_size(
        &self,
        num_identities: usize,
    ) -> anyhow::Result<usize> {
        Ok(self
            .get_suitable_insertion_prover(num_identities)
            .await?
            .batch_size())
    }

    pub async fn get_suitable_insertion_prover(
        &self,
        num_identities: usize,
    ) -> anyhow::Result<RwLockReadGuard<Prover>> {
        let prover_map = self.insertion_prover_map.read().await;

        match RwLockReadGuard::try_map(prover_map, |map| map.get(num_identities)) {
            Ok(p) => anyhow::Ok(p),
            Err(_) => Err(anyhow!(
                "No available prover for batch size: {num_identities}"
            )),
        }
    }

    pub async fn get_suitable_deletion_prover(
        &self,
        num_identities: usize,
    ) -> anyhow::Result<RwLockReadGuard<Prover>> {
        let prover_map = self.deletion_prover_map.read().await;

        match RwLockReadGuard::try_map(prover_map, |map| map.get(num_identities)) {
            Ok(p) => anyhow::Ok(p),
            Err(_) => Err(anyhow!(
                "No available prover for batch size: {num_identities}"
            )),
        }
    }
}
