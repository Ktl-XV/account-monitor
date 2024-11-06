use async_trait::async_trait;
use ethers::core::types::{
    transaction::eip2718::TypedTransaction, BlockId, BlockNumber, TransactionReceipt, U256, U64,
};
use ethers::providers::{Middleware, MiddlewareError};
use thiserror::Error;
/// A middleware allows customizing requests send and received from an ethereum node.
///
/// Writing a middleware is as simple as:
/// 1. implementing the [`inner`](crate::Middleware::inner) method to point to the next layer in the "middleware onion",
/// 2. implementing the [`MiddlewareError`](crate::MiddlewareError) trait on your middleware's error type
/// 3. implementing any of the methods you want to override
///

#[derive(Debug)]
pub struct CustomBlockReceiptsMiddleware<M> {
    inner: M,
}

#[derive(Error, Debug)]
pub enum CustomBlockReceiptsMiddlewareError<M: Middleware> {
    #[error("{0}")]
    MiddlewareError(M::Error),
}

impl<M: Middleware> MiddlewareError for CustomBlockReceiptsMiddlewareError<M> {
    type Inner = M::Error;

    fn from_err(src: M::Error) -> CustomBlockReceiptsMiddlewareError<M> {
        CustomBlockReceiptsMiddlewareError::MiddlewareError(src)
    }

    fn as_inner(&self) -> Option<&Self::Inner> {
        match self {
            CustomBlockReceiptsMiddlewareError::MiddlewareError(e) => Some(e),
            _ => None,
        }
    }
}

impl<M> CustomBlockReceiptsMiddleware<M>
where
    M: Middleware,
{
    /// Creates an instance of GasMiddleware
    /// `ìnner` the inner Middleware
    /// `perc` This is an unsigned integer representing the percentage increase in the amount of gas
    /// to be used for the transaction. The percentage is relative to the gas value specified in the
    /// transaction. Valid contingency values are in range 1..=50. Otherwise a custom middleware
    /// error is raised.
    pub fn new(inner: M) -> Result<Self, CustomBlockReceiptsMiddlewareError<M>> {
        Ok(Self { inner })
    }
}

#[async_trait]
impl<M> Middleware for CustomBlockReceiptsMiddleware<M>
where
    M: Middleware,
{
    type Error = CustomBlockReceiptsMiddlewareError<M>;
    type Provider = M::Provider;
    type Inner = M;

    fn inner(&self) -> &M {
        &self.inner
    }

    /// Overrides the default `get_block_number` method to always return 0
    async fn get_block_number(&self) -> Result<U64, Self::Error> {
        Ok(U64::zero())
    }

    /// Overrides the default `estimate_gas` method to log that it was called,
    /// before forwarding the call to the next layer.
    async fn estimate_gas(
        &self,
        tx: &TypedTransaction,
        block: Option<BlockId>,
    ) -> Result<U256, Self::Error> {
        println!("Estimating gas...");
        self.inner()
            .estimate_gas(tx, block)
            .await
            .map_err(MiddlewareError::from_err)
    }
}
