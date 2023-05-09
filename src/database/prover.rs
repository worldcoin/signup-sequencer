use sqlx::FromRow;

pub type Provers = Vec<Prover>;

#[derive(FromRow)]
pub struct Prover {
    pub url:        String,
    pub batch_size: i64,
    pub timeout_s:  i64,
}
