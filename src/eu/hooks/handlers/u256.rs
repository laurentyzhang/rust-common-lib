use super::super::{CallInput, CallOutput, CallScheme};
use super::CallHandler;
use crate::crdt::{self, Crdt, state::DeltaOp};
use crate::eu::{CallHookError, ReadRecorder, ScalarAdapter, ScalarKeyCodec, ScalarKind, key::Key};
use alloy_evm::EvmInternals;
use alloy_primitives::U256;
use alloy_sol_types::{SolInterface, SolValue, sol};

sol! {
    interface IU256 {
        function init(bytes32 key, uint256 min, uint256 max) external returns (bytes32);
        function add(bytes32 key, uint256 delta) external returns (bool);
        function sub(bytes32 key, uint256 delta) external returns (bool);
        function get(bytes32 key) external view returns (uint256);
    }
}

/// Unsigned 256-bit scalar operations backed by the caller's storage.
pub struct U256Handler;

impl U256Handler {
    fn update(
        &self,
        key: U256,
        delta: DeltaOp<U256>,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError> {
        if input.scheme == CallScheme::StaticCall {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }
        let (mut scalar, slots) =
            ScalarAdapter::new(key).load::<U256, _>(internals, input.caller)?;
        if scalar.add_delta(&delta).is_err() {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }
        slots.store(internals, input.caller, &scalar)?;
        Ok(CallOutput::returned(0, true.abi_encode()))
    }
}

impl CallHandler for U256Handler {
    fn is_valid_slot(&self, slot: U256) -> bool {
        matches!(ScalarKeyCodec::decode(slot), Some((ScalarKind::U256, _, _)))
    }

    fn call(
        &self,
        key: U256,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        api_reads: &mut ReadRecorder,
    ) -> Result<CallOutput, CallHookError> {
        if !self.is_valid_slot(key) {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }
        match IU256::IU256Calls::abi_decode_validate(input.input) {
            Ok(IU256::IU256Calls::init(call)) => {
                if input.scheme == CallScheme::StaticCall || call.min > call.max {
                    return Ok(CallOutput::reverted(0, Vec::new()));
                }
                let scalar = crdt::Scalar::<U256> {
                    min: call.min,
                    max: call.max,
                    ..Default::default()
                };
                ScalarAdapter::new(key).store(internals, input.caller, &scalar)?;
                Ok(CallOutput::returned(0, call.key.abi_encode()))
            }
            Ok(IU256::IU256Calls::add(call)) => {
                self.update(key, DeltaOp::Add(call.delta), input, internals)
            }
            Ok(IU256::IU256Calls::sub(call)) => {
                self.update(key, DeltaOp::Sub(call.delta), input, internals)
            }
            Ok(IU256::IU256Calls::get(_)) => {
                let (mut scalar, slots) =
                    ScalarAdapter::new(key).load::<U256, _>(internals, input.caller)?;
                // Record each field used to compute the observable value.
                for slot in [slots.value, slots.pos_delta, slots.neg_delta] {
                    api_reads.record(Key::Storage(input.caller, slot));
                }
                Ok(CallOutput::returned(
                    0,
                    scalar.apply_delta().value.abi_encode(),
                ))
            }
            Err(_) => Ok(CallOutput::reverted(0, Vec::new())),
        }
    }
}
