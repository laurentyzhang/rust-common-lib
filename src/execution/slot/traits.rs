//! Traits implemented by the CRDT namespaces stored in EVM slots.

use std::fmt::Debug;

use alloy_evm::revm::state::EvmStorage;
use alloy_primitives::{Address, U256};

use crate::common::codec::CodecError;
use crate::store::SlotStore;
use crate::execution::workload::ID;
use crate::store::StoreError;

/// Collects accepted states' changes to one CRDT namespace and applies them once onto the shared
/// EVM state; every CRDT namespace has its own applier.
pub trait ApplyDelta {
    /// Pending changes of one owner in this namespace, collected from accepted states.
    type Delta: Default;

    /// Reads one accepted state's `slots` in this namespace into `delta`, tagged with the state's
    /// `id`; touches no store.
    fn merge_delta_from(delta: &mut Self::Delta, id: ID, slots: &EvmStorage) -> Result<(), CodecError>;

    /// Writes `owner`'s collected `delta` into the committed values in `store`.
    fn apply<S: SlotStore>(store: &mut S, owner: Address, delta: Self::Delta) -> Result<(), StoreError>
    where
        S::Error: Debug;
}

/// Converts one CRDT namespace between its compact format and its indexed format; each namespace
/// knows its own keys and layout.
pub trait FormatConverter {
    /// Expands `owner`'s value that `key` belongs to into the indexed format; does nothing once
    /// expanded.
    fn expand<S: SlotStore>(store: &mut S, owner: Address, key: U256) -> Result<(), S::Error>
    where
        S::Error: From<CodecError>;

    /// Returns `owner`'s changed values among `slots` in the compact format, reading what `slots`
    /// doesn't hold from `store`.
    fn compact<S: SlotStore>(
        store: &mut S,
        owner: Address,
        slots: &EvmStorage,
    ) -> Result<EvmStorage, StoreError>
    where
        S::Error: Debug;
}
