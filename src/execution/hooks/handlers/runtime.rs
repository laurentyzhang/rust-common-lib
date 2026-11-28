use super::super::{CallInput, CallOutput, CallScheme};
use super::CallHandler;
use crate::execution::key::Key;
use crate::execution::{CallHookError, Recorder};
use crate::planner::meta::{MetaCodec, MetaFieldId};
use crate::store::SlotStore;
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
        matches!(MetaCodec::decode(slot), Some((property, _, _, _)) if property == MetaFieldId::DeferredGasLimit as u8)
    }

    fn call(
        &self,
        _key: U256,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        api_recorder: &mut Recorder,
    ) -> Result<CallOutput, CallHookError> {
        match IRuntime::IRuntimeCalls::abi_decode_validate(input.input) {
            Ok(IRuntime::IRuntimeCalls::setDeferral(call)) => {
                if input.scheme != CallScheme::Call {
                    return Ok(CallOutput::reverted(0, Vec::new()));
                }

                // No actual read is performed here, we are just logging the intent to read.
                api_recorder.log_read(Key::Account(input.caller));

                // Make sure the account exists first.
                let caller = internals
                    .load_account(input.caller)
                    .map_err(|error| CallHookError::Handler(error.to_string()))?;

                // Runtime code is installed only after the constructor finishes.
                if !caller.data.is_created_locally()
                    || caller.data.info.code_hash != KECCAK256_EMPTY
                {
                    return Ok(CallOutput::reverted(0, Vec::new()));
                }
                let slot = MetaFieldId::DeferredGasLimit.slot(input.caller, call.selector.0, 0);

                // Log that we are writing to the caller's account.
                api_recorder.log_write(Key::Storage(input.caller, slot));
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
