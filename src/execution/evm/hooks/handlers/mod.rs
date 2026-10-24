mod scalar;

pub use scalar::Scalar;

use alloy_evm::EvmInternals;

use super::{CallInput, CallOutput};
use crate::execution::evm::CallHookError;

/// Handles calls for one reserved address.
pub trait CallHandler: Send + Sync {
    /// Executes the API call, returning output or a handler failure.
    fn call(
        &self,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError>;
}

impl<F> CallHandler for F
where
    F: Fn(&CallInput<'_>) -> Result<CallOutput, CallHookError> + Send + Sync,
{
    /// Invokes the closure directly, without a handler wrapper.
    fn call(
        &self,
        input: &CallInput<'_>,
        _internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError> {
        self(input)
    }
}
