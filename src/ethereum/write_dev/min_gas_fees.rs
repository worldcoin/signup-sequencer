use async_trait::async_trait;
use core::cmp::max;
use ethers::{
    middleware::gas_oracle::{GasOracle, GasOracleError},
    types::U256,
};

#[derive(Debug, Clone)]
pub struct MinGasFees<Inner> {
    inner: Inner,
    min_max_fee: U256,
    min_priority_fee: U256,
    priority_fee_multiplier_percentage: U256,
}

impl<Inner> MinGasFees<Inner> {
    pub const fn new(
        inner: Inner,
        min_max_fee: U256,
        min_priority_fee: U256,
        priority_fee_multiplier_percentage: U256,
    ) -> Self {
        Self {
            inner,
            min_max_fee,
            min_priority_fee,
            priority_fee_multiplier_percentage,
        }
    }
}

#[async_trait]
impl<Inner: GasOracle> GasOracle for MinGasFees<Inner> {
    async fn fetch(&self) -> Result<U256, GasOracleError> {
        // Only supports eip1559.
        self.inner.fetch().await
    }

    async fn estimate_eip1559_fees(&self) -> Result<(U256, U256), GasOracleError> {
        let (max_fee, priority_fee) = self.inner.estimate_eip1559_fees().await?;
        let priority_fee = priority_fee * (self.priority_fee_multiplier_percentage / 100);
        let max_fee = max(self.min_max_fee, max_fee);
        let priority_fee = max(self.min_priority_fee, priority_fee);
        Ok((max_fee, priority_fee))
    }
}
