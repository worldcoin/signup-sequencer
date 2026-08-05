use crate::prover::{Prover, ProverMap};
use anyhow::anyhow;
use tokio::sync::{RwLock, RwLockReadGuard};

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

    pub async fn has_insertion_provers(&self) -> bool {
        !self.insertion_prover_map.read().await.is_empty()
    }

    pub async fn has_deletion_provers(&self) -> bool {
        !self.deletion_prover_map.read().await.is_empty()
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
