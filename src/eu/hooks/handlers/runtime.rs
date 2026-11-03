use super::super::{CallInput, CallOutput, CallScheme};
use super::CallHandler;
use crate::eu::{CallHookError, ReadRecorder, state::ApiStore};
use crate::planner::meta::{MetaKeyCodec, property::Property};
use alloy_evm::EvmInternals;
use alloy_primitives::{KECCAK256_EMPTY, U256};
use alloy_sol_types::{SolInterface, SolValue, sol};

sol! {
    interface IRuntime {
        function setDeferral(bytes4 selector, uint64 prepaidGas) external;
        function isDeferred() external view returns (bool);
    }
}

/// Configures function deferral and reports the current job's execution phase.
pub struct RuntimeHandler;

impl CallHandler for RuntimeHandler {
    fn is_valid_slot(&self, slot: U256) -> bool {
        matches!(MetaKeyCodec::decode(slot), Some((property, _, _, _)) if property == Property::DeferredGas as u8)
    }

    fn call(
        &self,
        _key: U256,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        _api_reads: &mut ReadRecorder,
    ) -> Result<CallOutput, CallHookError> {
        match IRuntime::IRuntimeCalls::abi_decode_validate(input.input) {
            Ok(IRuntime::IRuntimeCalls::setDeferral(call)) => {
                if input.scheme != CallScheme::Call {
                    return Ok(CallOutput::reverted(0, Vec::new()));
                }
                // Runtime code is installed only after the constructor finishes.
                let caller = internals
                    .load_account(input.caller)
                    .map_err(|error| CallHookError::Handler(error.to_string()))?;
                if !caller.data.is_created_locally()
                    || caller.data.info.code_hash != KECCAK256_EMPTY
                {
                    return Ok(CallOutput::reverted(0, Vec::new()));
                }
                let mut callee = [0; 8];
                callee[..4].copy_from_slice(&input.caller[..4]);
                callee[4..].copy_from_slice(call.selector.as_slice());
                let slot = Property::DeferredGas.slot(input.caller, u64::from_le_bytes(callee), 0);
                internals.write(input.caller, slot, U256::from(call.prepaidGas))?;
                Ok(CallOutput::returned(0, Vec::new()))
            }
            Ok(IRuntime::IRuntimeCalls::isDeferred(_)) => {
                Ok(CallOutput::returned(0, input.is_deferred.abi_encode()))
            }
            Err(_) => Ok(CallOutput::reverted(0, Vec::new())),
        }
    }
}
