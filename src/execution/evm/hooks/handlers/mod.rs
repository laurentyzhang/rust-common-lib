mod scalar;

#[cfg(test)]
mod tests;

pub use scalar::ScalarHandler;

use alloy_evm::EvmInternals;
use alloy_primitives::U256;

use super::{CallInput, CallOutput};
use crate::execution::evm::{CallHookError, ReadRecorder};

/// Handles calls for one reserved address.
pub trait CallHandler: Send + Sync {
    /// Executes the API call, returning output or a handler failure.
    fn call(
        &self,
        key: U256,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        read_recorder: &mut ReadRecorder,
    ) -> Result<CallOutput, CallHookError>;

    /// Recognizes storage slots owned by this API, for any handle.
    fn is_valid_slot(&self, slot: U256) -> bool;
}

impl<F> CallHandler for F
where
    F: Fn(&CallInput<'_>) -> Result<CallOutput, CallHookError> + Send + Sync,
{
    /// Invokes the closure directly, without a handler wrapper.
    fn call(
        &self,
        _key: U256,
        input: &CallInput<'_>,
        _internals: &mut EvmInternals<'_>,
        _read_recorder: &mut ReadRecorder,
    ) -> Result<CallOutput, CallHookError> {
        self(input)
    }

    /// Call-only closures do not define a storage layout.
    fn is_valid_slot(&self, _slot: U256) -> bool {
        false
    }
}
