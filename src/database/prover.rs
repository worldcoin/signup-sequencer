pub type Provers = Vec<Prover>;

pub struct Prover {
    pub url:        String,
    pub batch_size: u64,
    pub timeout_s:  u64,
}

impl Prover {
    pub async fn new(url: impl ToString, batch_size: u64, timeout_s: u64) -> Prover {
        Self {
            url: url.to_string(),
            batch_size,
            timeout_s,
        }
    }
}
