use crate::utils::u256_to_f64;
use ::prometheus::{register_gauge, Gauge};
use async_trait::async_trait;
use ethers::{
    middleware::gas_oracle::{GasOracle, GasOracleError},
    types::U256,
};
use once_cell::sync::Lazy;
use tracing::debug;

static GAS_PRICE: Lazy<Gauge> =
    Lazy::new(|| register_gauge!("eth_gas_price", "Ethereum gas price for transactions.").unwrap());
static MAX_FEE: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!(
        "eth_max_fee",
        "Ethereum EIP1559 maximum gas fee for transactions."
    )
    .unwrap()
});
static PRIORITY_FEE: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!(
        "eth_priority_fee",
        "Ethereum EIP1559 priority gas fee for transactions."
    )
    .unwrap()
});

#[derive(Debug, Clone)]
pub struct GasOracleLogger<Inner> {
    inner: Inner,
}

impl<Inner> GasOracleLogger<Inner> {
    pub const fn new(inner: Inner) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<Inner: GasOracle> GasOracle for GasOracleLogger<Inner> {
    async fn fetch(&self) -> Result<U256, GasOracleError> {
        let value = self.inner.fetch().await?;
        GAS_PRICE.set(u256_to_f64(value));
        debug!(gas_price = ?value, "Fetched legacy gas price");
        Ok(value)
    }

    async fn estimate_eip1559_fees(&self) -> Result<(U256, U256), GasOracleError> {
        let (max_fee, priority_fee) = self.inner.estimate_eip1559_fees().await?;
        MAX_FEE.set(u256_to_f64(max_fee));
        PRIORITY_FEE.set(u256_to_f64(priority_fee));
        debug!(?max_fee, ?priority_fee, "Fetched gas price");
        Ok((max_fee, priority_fee))
    }
}
