use alloy_evm::EvmInternals;

use super::super::{CallInput, CallOutput};
use super::CallHandler;
use crate::execution::evm::CallHookError;

pub struct Scalar;

impl CallHandler for Scalar {
    fn call(
        &self,
        _input: &CallInput<'_>,
        _internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError> {
        // Scalar calls arrive here.
        Ok(CallOutput::reverted(0, Vec::new()))
    }
}
