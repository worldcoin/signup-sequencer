pub type Provers = Vec<Prover>;

pub struct Prover {
    pub url:        String,
    pub batch_size: usize,
    pub timeout_s:  u64,
}
