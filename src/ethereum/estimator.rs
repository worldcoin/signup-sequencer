use async_trait::async_trait;
use ethers::{
    providers::{FromErr, Middleware, PendingTransaction},
    types::{transaction::eip2718::TypedTransaction, u256_from_f64_saturating, BlockId, U256},
};
use thiserror::Error;

/// Estimator Provider Errors
#[derive(Error, Debug)]
#[allow(clippy::module_name_repetitions)]
pub enum EstimatorError<Inner: Middleware> {
    #[error("{0}")]
    /// Thrown when an internal middleware errors
    MiddlewareError(Inner::Error),
}

// Boilerplate
impl<Inner: Middleware> FromErr<Inner::Error> for EstimatorError<Inner> {
    fn from(src: Inner::Error) -> Self {
        Self::MiddlewareError(src)
    }
}

#[derive(Debug)]
/// Middleware used for setting gas limit. Uses the gas limit from
/// the inner middleware, scales it by a factor and adds extra gas.
pub struct Estimator<Inner> {
    inner: Inner,
    scale: f64,
    extra: f64,
}

impl<Inner: Middleware> Estimator<Inner> {
    pub const fn new(inner: Inner, scale: f64, extra: f64) -> Self {
        Self {
            inner,
            scale,
            extra,
        }
    }

    #[allow(clippy::cast_precision_loss)]
    #[allow(clippy::cast_lossless)]
    pub fn rescale(&self, gas: U256) -> U256 {
        let gas = {
            let n = (256_u32 - 64).saturating_sub(gas.leading_zeros());
            let gas = (gas >> n).as_u64() as f64;
            gas * (n as f64).exp2()
        };
        let gas = gas.mul_add(self.scale, self.extra);
        u256_from_f64_saturating(gas)
    }
}

#[async_trait]
impl<Inner: Middleware> Middleware for Estimator<Inner> {
    type Error = EstimatorError<Inner>;
    type Inner = Inner;
    type Provider = Inner::Provider;

    fn inner(&self) -> &Inner {
        &self.inner
    }

    async fn fill_transaction(
        &self,
        tx: &mut TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<(), Self::Error> {
        if tx.gas().is_none() {
            tx.set_gas(self.estimate_gas(tx, block).await?);
        }
        self.inner()
            .fill_transaction(tx, block)
            .await
            .map_err(FromErr::from)
    }

    async fn estimate_gas(
        &self,
        tx: &TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<U256, Self::Error> {
        let gas = self
            .inner
            .estimate_gas(tx, block)
            .await
            .map_err(FromErr::from)?;
        Ok(self.rescale(gas))
    }

    /// Signs and broadcasts the transaction. The optional parameter `block` can
    /// be passed so that gas cost and nonce calculations take it into
    /// account. For simple transactions this can be left to `None`.
    async fn send_transaction<T: Into<TypedTransaction> + Send + Sync>(
        &self,
        tx: T,
        block: Option<BlockId>,
    ) -> Result<PendingTransaction<'_, Self::Provider>, Self::Error> {
        let mut tx = tx.into();
        if tx.gas().is_none() {
            tx.set_gas(self.estimate_gas(&tx, block).await?);
        }
        self.inner
            .send_transaction(tx, block)
            .await
            .map_err(FromErr::from)
    }
}
