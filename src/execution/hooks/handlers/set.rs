use super::super::{CallInput, CallOutput, CallScheme};
use super::CallHandler;
use crate::execution::{CallHookError, FormatConverter, Recorder, key::Key, set};
use alloy_evm::EvmInternals;
use alloy_primitives::U256;
use alloy_sol_types::{SolInterface, SolValue, sol};

sol! {
    interface ISet {
        function insert(bytes32 set, uint256 element) external returns (bool);
        function remove(bytes32 set, uint256 element) external returns (bool);
        function contains(bytes32 set, uint256 element) external view returns (bool);
        function length(bytes32 set) external view returns (uint256);
        function first(bytes32 set) external view returns (bool, uint256);
        function next(bytes32 set, uint256 element) external view returns (bool, uint256);
    }
}

/// Ordered U256-element sets owned by the caller, one per set key. Calls work on the execution
/// format plus the pending changes; first/next visit the index, then pending additions, skipping
/// removed ones.
pub struct SetHandler;

impl CallHandler for SetHandler {
    /// Recognizes Set-namespace slots with a defined property.
    fn is_valid_slot(&self, slot: U256) -> bool {
        set::KeyCodec::decode(slot).is_some()
    }

    /// Executes ISet calls on the caller's set named by the router's key (the first argument).
    fn call(
        &self,
        key: U256,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
        recorder: &mut Recorder,
    ) -> Result<CallOutput, CallHookError> {
        let owner = input.caller;
        // Map the caller's set key to its private set ID.
        let set = set::KeyCodec::derive_key(key);
        let length = set::KeyCodec::length(set);
        // The first touch in the block converts the list (storage format) into the execution format.
        set::Converter::to_execution_format(internals, owner, set)?;
        match ISet::ISetCalls::abi_decode_validate(input.input) {
            Ok(ISet::ISetCalls::insert(_) | ISet::ISetCalls::remove(_))
                if input.scheme == CallScheme::StaticCall =>
            {
                Ok(CallOutput::reverted(0, Vec::new()))
            }
            // Stage the change in the pending delta; returns whether it changed. Length is a delta for readers.
            Ok(ISet::ISetCalls::insert(call)) => {
                let changed = set::Indexer::insert(internals, owner, set, call.element)?;
                recorder.log_delta(Key::Storage(owner, length));
                Ok(CallOutput::returned(0, changed.abi_encode()))
            }
            Ok(ISet::ISetCalls::remove(call)) => {
                let changed = set::Indexer::remove(internals, owner, set, call.element)?;
                recorder.log_delta(Key::Storage(owner, length));
                Ok(CallOutput::returned(0, changed.abi_encode()))
            }
            // Index membership plus pending flags; a pending add wins over a pending remove.
            Ok(ISet::ISetCalls::contains(call)) => {
                for property in [set::Property::Node, set::Property::Added, set::Property::Removed] {
                    let key = set::KeyCodec::element(property, set, call.element);
                    recorder.log_read(Key::Storage(owner, key));
                }
                let found = set::Indexer::contains(internals, owner, set, call.element)?;
                Ok(CallOutput::returned(0, found.abi_encode()))
            }
            // Live element count; pending logs are this transaction's own data, so only Length is recorded.
            Ok(ISet::ISetCalls::length(_)) => {
                recorder.log_read(Key::Storage(owner, length));
                let count = set::Indexer::length(internals, owner, set)?;
                Ok(CallOutput::returned(0, count.abi_encode()))
            }
            // Cursor traversal over live elements: index order, then pending additions.
            Ok(ISet::ISetCalls::first(_)) => {
                recorder.log_read(Key::Storage(owner, length));
                let first = set::Indexer::first(internals, owner, set)?;
                Ok(CallOutput::returned(0, (first.is_some(), first.unwrap_or_default()).abi_encode_params()))
            }
            Ok(ISet::ISetCalls::next(call)) => {
                recorder.log_read(Key::Storage(owner, length));
                let next = set::Indexer::next(internals, owner, set, call.element)?;
                Ok(CallOutput::returned(0, (next.is_some(), next.unwrap_or_default()).abi_encode_params()))
            }
            Err(_) => Ok(CallOutput::reverted(0, Vec::new())),
        }
    }
}
