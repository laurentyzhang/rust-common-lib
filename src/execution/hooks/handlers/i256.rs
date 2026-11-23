use super::super::{CallInput, CallOutput, CallScheme};
use super::CallHandler;
use crate::crdt::{self, Crdt, state::DeltaOp};
use crate::execution::{
    CallHookError, Recorder, ScalarAdapter, ScalarKeyCodec, ScalarKind, key::Key,
};
use crate::namespace::Namespace;
use alloy_evm::EvmInternals;
use alloy_primitives::{I256, U256};
use alloy_sol_types::{SolInterface, SolValue, sol};

sol! {
    interface II256 {
        function init(bytes32 key, int256 min, int256 max) external returns (bytes32);
        function add(bytes32 key, uint256 delta) external returns (bool);
        function sub(bytes32 key, uint256 delta) external returns (bool);
        function get(bytes32 key) external view returns (int256);
    }
}

/// Signed 256-bit scalar operations backed by the caller's storage.
pub struct I256Handler;

impl I256Handler {
    fn update_delta(
        &self,
        key: U256,
        delta: DeltaOp<U256>,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        recorder: &mut Recorder,
    ) -> Result<CallOutput, CallHookError> {
        if input.scheme == CallScheme::StaticCall {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }
        let (mut scalar, slots) =
            ScalarAdapter::new(key).load::<I256, _>(internals, input.caller, recorder)?;
        if scalar.add_delta(&delta).is_err() {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }
        recorder.log_delta(Key::Storage(input.caller, slots.value));
        slots.store(internals, input.caller, &scalar, recorder)?;
        Ok(CallOutput::returned(0, true.abi_encode()))
    }
}

impl CallHandler for I256Handler {
    fn is_valid_slot(&self, slot: U256) -> bool {
        matches!(ScalarKeyCodec::decode(slot), Some((ScalarKind::I256, _, _)))
    }

    fn call(
        &self,
        key: U256,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        api_reads: &mut Recorder,
    ) -> Result<CallOutput, CallHookError> {
        // Map the caller's key to its internal scalar key.
        let key = ScalarKeyCodec::derive_key(Namespace::I256, key);
        match II256::II256Calls::abi_decode_validate(input.input) {
            Ok(II256::II256Calls::init(call)) => {
                if input.scheme == CallScheme::StaticCall || call.min > call.max {
                    return Ok(CallOutput::reverted(0, Vec::new()));
                }
                let scalar = crdt::Scalar::<I256> {
                    min: call.min,
                    max: call.max,
                    ..Default::default()
                };
                ScalarAdapter::new(key).store(internals, input.caller, &scalar, api_reads)?;
                Ok(CallOutput::returned(0, call.key.abi_encode()))
            }
            Ok(II256::II256Calls::add(call)) => {
                self.update_delta(key, DeltaOp::Add(call.delta), input, internals, api_reads)
            }
            Ok(II256::II256Calls::sub(call)) => {
                self.update_delta(key, DeltaOp::Sub(call.delta), input, internals, api_reads)
            }
            Ok(II256::II256Calls::get(_)) => {
                let (mut scalar, slots) =
                    ScalarAdapter::new(key).load::<I256, _>(internals, input.caller, api_reads)?;
                api_reads.log_write(Key::Storage(input.caller, slots.value));
                Ok(CallOutput::returned(
                    0,
                    scalar.apply_delta().value.abi_encode(),
                ))
            }
            Err(_) => Ok(CallOutput::reverted(0, Vec::new())),
        }
    }
}
