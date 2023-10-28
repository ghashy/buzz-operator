use std::error::Error;

use async_trait::async_trait;

pub mod service_bunch;
pub mod service_unit;
mod unit_connection;

/// This trait generalizes abstract service behavior.
///
/// You should strictly follow the order of calling functions:
/// 1. `run`
/// 2. `wait_on`
#[async_trait]
pub trait Service<E>
where
    E: Error,
{
    type Output;
    /// This function should actually run service execution.
    fn run(&mut self) -> std::result::Result<Self::Output, E>;
    /// This function should wait until the service exits itself, or terminated.
    async fn wait_on(&mut self) -> Result<(), E>;
}
