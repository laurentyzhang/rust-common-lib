mod i256;
mod runtime;
mod u256;

#[cfg(test)]
mod tests;

pub use i256::{I256Handler, II256};
pub use runtime::{IRuntime, RuntimeHandler};
pub use u256::{IU256, U256Handler};

use alloy_evm::EvmInternals;
use alloy_primitives::U256;

use super::{CallInput, CallOutput};
use crate::execution::{CallHookError, ReadRecorder};

/// Handles calls for one reserved address.
pub trait CallHandler: Send + Sync {
    /// Executes the API call, returning output or a handler failure.
    fn call(
        &self,
        key: U256,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        api_reads: &mut ReadRecorder,
    ) -> Result<CallOutput, CallHookError>;

    /// Recognizes storage slots owned by this API, for any handle.
    fn is_valid_slot(&self, slot: U256) -> bool;
}

impl<F> CallHandler for F
where
    F: Fn(&CallInput<'_>) -> Result<CallOutput, CallHookError> + Send + Sync,
{
    fn call(
        &self,
        _key: U256,
        input: &CallInput<'_>,
        _internals: &mut EvmInternals<'_>,
        _api_reads: &mut ReadRecorder,
    ) -> Result<CallOutput, CallHookError> {
        self(input)
    }

    fn is_valid_slot(&self, _slot: U256) -> bool {
        false
    }
}
