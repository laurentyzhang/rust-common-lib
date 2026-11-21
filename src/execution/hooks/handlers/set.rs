use super::super::{CallInput, CallOutput, CallScheme};
use super::CallHandler;
use crate::execution::evm::storage::ApiStore;
use crate::execution::{CallHookError, Recorder, SetAdapter, SetKeyCodec, SetProperty, key::Key};
use alloy_evm::EvmInternals;
use alloy_primitives::{Bytes, U256};
use alloy_sol_types::{SolInterface, SolValue, sol};

sol! {
    interface ISet {
        function insert(bytes element) external returns (bool);
        function remove(bytes element) external returns (bool);
        function contains(bytes element) external view returns (bool);
        function length() external view returns (uint256);
        function at(uint256 index) external view returns (bool, bytes);
    }
}

/// Ordered byte-element set owned by the caller; pending inserts and removes get indexes
/// on the next index read (`length`/`at`).
pub struct SetHandler;

impl CallHandler for SetHandler {
    /// Recognizes Set-namespace slots with a defined property.
    fn is_valid_slot(&self, slot: U256) -> bool {
        SetKeyCodec::decode(slot).is_some()
    }

    /// Executes ISet calls on the caller's set; the router's key argument is unused.
    fn call(
        &self,
        _key: U256,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        recorder: &mut Recorder,
    ) -> Result<CallOutput, CallHookError> {
        let owner = input.caller;
        let length = SetKeyCodec::encode(SetProperty::Length, [0; 24]);
        match ISet::ISetCalls::abi_decode_validate(input.input) {
            Ok(ISet::ISetCalls::insert(_) | ISet::ISetCalls::remove(_))
                if input.scheme == CallScheme::StaticCall =>
            {
                Ok(CallOutput::reverted(0, Vec::new()))
            }
            // Store the element and flag it pending; Length is a delta for readers.
            Ok(ISet::ISetCalls::insert(call)) => {
                let id = SetKeyCodec::element_id(&call.element);
                SetAdapter::write_element(internals, owner, id, &call.element)?;
                SetAdapter::flag(internals, owner, id, SetProperty::Added)?;
                recorder.log_delta(Key::Storage(owner, length));
                Ok(CallOutput::returned(0, true.abi_encode()))
            }
            Ok(ISet::ISetCalls::remove(call)) => {
                let id = SetKeyCodec::element_id(&call.element);
                SetAdapter::flag(internals, owner, id, SetProperty::Removed)?;
                recorder.log_delta(Key::Storage(owner, length));
                Ok(CallOutput::returned(0, true.abi_encode()))
            }
            // Committed membership plus pending flags; a pending add wins over a pending remove.
            Ok(ISet::ISetCalls::contains(call)) => {
                let id = SetKeyCodec::element_id(&call.element);
                let [member, added, removed] = [
                    SetProperty::Member,
                    SetProperty::Added,
                    SetProperty::Removed,
                ]
                .map(|property| SetKeyCodec::encode(property, id));
                for key in [member, added, removed] {
                    recorder.log_read(Key::Storage(owner, key));
                }
                let found = !internals.read(owner, added)?.is_zero()
                    || (!internals.read(owner, member)?.is_zero()
                        && internals.read(owner, removed)?.is_zero());
                Ok(CallOutput::returned(0, found.abi_encode()))
            }
            // Index reads fold pending elements first, so they get indexes now.
            Ok(ISet::ISetCalls::length(_)) => {
                recorder.log_read(Key::Storage(owner, length));
                SetAdapter::fold_pending(internals, owner)
                    .map_err(|error| CallHookError::Handler(error.to_string()))?;
                Ok(CallOutput::returned(
                    0,
                    internals.read(owner, length)?.abi_encode(),
                ))
            }
            // Element at an index; found is false for a removed or out-of-range slot.
            Ok(ISet::ISetCalls::at(call)) => {
                recorder.log_read(Key::Storage(owner, length));
                SetAdapter::fold_pending(internals, owner)
                    .map_err(|error| CallHookError::Handler(error.to_string()))?;
                let mut output = (false, Bytes::new());
                if call.index < internals.read(owner, length)? {
                    let word = internals.read(owner, SetKeyCodec::index(call.index.to()))?;
                    if !word.is_zero() {
                        let id = word.to_be_bytes::<32>()[8..].try_into().unwrap();
                        output = (true, SetAdapter::read_element(internals, owner, id)?.into());
                    }
                }
                Ok(CallOutput::returned(0, output.abi_encode_params()))
            }
            Err(_) => Ok(CallOutput::reverted(0, Vec::new())),
        }
    }
}
