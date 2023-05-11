pub type Provers = Vec<ProverConfiguration>;

pub struct ProverConfiguration {
    pub url:        String,
    pub batch_size: usize,
    pub timeout_s:  u64,
}
