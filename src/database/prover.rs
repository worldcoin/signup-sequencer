use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
};

pub type Provers = HashSet<ProverConfiguration>;

#[derive(Debug, Clone, Eq, PartialEq)]
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
