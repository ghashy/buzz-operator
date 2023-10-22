use std::error::Error;

use async_trait::async_trait;

pub mod service_bunch;
pub mod service_unit;

// /// Service can exit with just Succes, or with a variety of Errors
// type Result<E> = std::result::Result<(), E>;

#[async_trait]
pub trait Service<T, E>
where
    E: Error,
{
    type Output;
    /// This function should actually run service executing.
    fn run(&mut self) -> Result<Self::Output, E>;
    /// This function waits until the service exits itself, or terminated.
    async fn wait_on(&mut self) -> Result<T, E>;
    // REVIEW: maybe &self is more appropriate?
    fn try_terminate(&mut self) -> Result<T, E>;
}
