use super::super::{
    CallInput, CallOutput, CallScheme,
    codec::{API_OFFSET, FIELD_OFFSET},
};
use super::CallHandler;
use crate::crdt::{self, Crdt, state::DeltaOp};
use crate::execution::evm::{
    ApiAddress, CallHookError, EvmInternalsStorage, Field, ReadRecorder, ScalarStorage, SlotCodec,
    key::Key,
};
use alloy_evm::EvmInternals;
use alloy_primitives::U256;
use alloy_sol_types::{SolInterface, SolValue, sol};

sol! {
    interface IScalar {
        function init(bytes32 key, uint256 min, uint256 max) external returns (bytes32);
        function add(bytes32 key, uint256 delta) external returns (bool);
        function sub(bytes32 key, uint256 delta) external returns (bool);
        function get(bytes32 key) external view returns (uint256);
    }
}

// Scalar handler for the IScalar interface
pub struct ScalarHandler;

impl ScalarHandler {
    /// Stores the bounds with zero value and delta under the caller-supplied key.
    fn init(
        &self,
        key: U256,
        call: IScalar::initCall,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError> {
        if input.scheme == CallScheme::StaticCall || call.min > call.max {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }

        let state = crdt::Scalar::<U256> {
            limits: (call.min, call.max),
            ..Default::default()
        };
        let scalar_storage = ScalarStorage::new(key);
        scalar_storage.store(
            &mut EvmInternalsStorage::new(internals),
            input.caller,
            &state,
        )?;
        Ok(CallOutput::returned(0, call.key.abi_encode()))
    }

    /// Validates and accumulates an addition without applying it to the value.
    fn add(
        &self,
        key: U256,
        call: IScalar::addCall,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError> {
        if input.scheme == CallScheme::StaticCall {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }

        let mut slot_storage = EvmInternalsStorage::new(internals);
        let (mut state, scalar_storage) =
            ScalarStorage::new(key).load(&mut slot_storage, input.caller)?;
        if state.add_delta(&DeltaOp::Add(call.delta)).is_err() {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }

        scalar_storage.store(&mut slot_storage, input.caller, &state)?;
        Ok(CallOutput::returned(0, true.abi_encode()))
    }

    /// Validates and accumulates a subtraction without applying it to the value.
    fn sub(
        &self,
        key: U256,
        call: IScalar::subCall,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError> {
        if input.scheme == CallScheme::StaticCall {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }

        let mut slot_storage = EvmInternalsStorage::new(internals);
        let (mut state, scalar_storage) =
            ScalarStorage::new(key).load(&mut slot_storage, input.caller)?;
        if state.add_delta(&DeltaOp::Sub(call.delta)).is_err() {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }

        scalar_storage.store(&mut slot_storage, input.caller, &state)?;
        Ok(CallOutput::returned(0, true.abi_encode()))
    }

    /// Materializes pending deltas and writes the scalar back to its original slots.
    fn get(
        &self,
        key: U256,
        _call: IScalar::getCall,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        api_reads: &mut ReadRecorder,
    ) -> Result<(U256, ScalarStorage), CallHookError> {
        let mut slot_storage = EvmInternalsStorage::new(internals);
        let (mut state, scalar_storage) =
            ScalarStorage::new(key).load(&mut slot_storage, input.caller)?;
        scalar_storage.store(&mut slot_storage, input.caller, state.apply_delta())?;
        api_reads.record(Key::Storage(input.caller, scalar_storage.value));
        Ok((state.value, scalar_storage))
    }
}

impl CallHandler for ScalarHandler {
    /// Recognizes scalar API slots with a defined field ID.
    fn is_valid_slot(&self, slot: U256) -> bool {
        let bytes = slot.to_be_bytes::<32>();
        SlotCodec::is_api_slot(slot)
            && bytes[API_OFFSET] == ApiAddress::Scalar as u8
            && Field::from_repr(bytes[FIELD_OFFSET]).is_some()
    }

    fn call(
        &self,
        key: U256,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        api_reads: &mut ReadRecorder,
    ) -> Result<CallOutput, CallHookError> {
        // Reject invalid keys before decoding or executing an operation.
        if !self.is_valid_slot(key) {
            return Ok(CallOutput::reverted(0, Vec::new()));
        }

        match IScalar::IScalarCalls::abi_decode_validate(input.input) {
            Ok(IScalar::IScalarCalls::init(call)) => self.init(key, call, input, internals),
            Ok(IScalar::IScalarCalls::add(call)) => self.add(key, call, input, internals),
            Ok(IScalar::IScalarCalls::sub(call)) => self.sub(key, call, input, internals),
            Ok(IScalar::IScalarCalls::get(call)) => {
                let (value, scalar_storage) = self.get(key, call, input, internals, api_reads)?;
                // Preserve the observation even when materialization changes no slot values.
                for slot in [scalar_storage.value, scalar_storage.delta] {
                    api_reads.record(Key::Storage(input.caller, slot));
                }
                Ok(CallOutput::returned(0, value.abi_encode()))
            }
            Err(_) => Ok(CallOutput::reverted(0, Vec::new())),
        }
    }
}
