use std::collections::HashSet;
use std::hash::{Hash, Hasher};

pub type Provers = HashSet<ProverConfiguration>;

#[derive(Debug, Clone)]
pub struct ProverConfiguration {
    pub url:        String,
    pub batch_size: usize,
    pub timeout_s:  u64,
}

impl Hash for ProverConfiguration {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.batch_size.hash(state);
    }
}

impl PartialEq for ProverConfiguration {
    fn eq(&self, other: &Self) -> bool {
        self.batch_size == other.batch_size
    }
}

impl Eq for ProverConfiguration {}
