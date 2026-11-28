//! Traits implemented by the CRDT namespaces stored in EVM slots.

use std::fmt::Debug;

use alloy_evm::revm::state::EvmStorage;
use alloy_primitives::{Address, U256};

use crate::common::codec::CodecError;
use crate::store::SlotStore;
use crate::execution::workload::ID;
use crate::store::StoreError;

/// Collects accepted states' changes to one CRDT namespace and writes them once into the shared
/// EVM state's pending deltas; only the persistent store applies them. Every CRDT namespace has
/// its own applier.
pub trait ApplyDelta {
    /// Pending changes of one owner in this namespace, collected from accepted states.
    type Delta: Default;

    /// Reads one accepted state's own changes over its base, among `slots` in this namespace, into
    /// `delta`, tagged with the state's `id`; touches no store.
    fn merge_delta_from(delta: &mut Self::Delta, id: ID, slots: &EvmStorage) -> Result<(), CodecError>;

    /// Writes `owner`'s collected `delta` into the pending deltas in `store`; the committed values
    /// stay unchanged.
    fn apply<S: SlotStore>(store: &mut S, owner: Address, delta: Self::Delta) -> Result<(), StoreError>
    where
        S::Error: Debug;
}

/// Converts one CRDT namespace between its compact format and its indexed format, and applies its
/// pending deltas at the persistent store; each namespace knows its own keys and layout.
pub trait FormatConverter {
    /// Pending changes of one owner in this namespace.
    type Delta;

    /// Expands `owner`'s value that `key` belongs to into the indexed format; does nothing once
    /// expanded.
    fn expand<S: SlotStore>(store: &mut S, owner: Address, key: U256) -> Result<(), S::Error>
    where
        S::Error: From<CodecError>;

    /// Applies `owner`'s `delta` to its committed values in `store` and returns the changed ones in
    /// the compact format.
    fn compact<S: SlotStore>(
        store: &mut S,
        owner: Address,
        delta: Self::Delta,
    ) -> Result<EvmStorage, StoreError>
    where
        S::Error: Debug;

    /// Applies `owner`'s `delta` to its committed values in `store` and returns the changed ones in
    /// the indexed format, without holes.
    fn reindex<S: SlotStore>(
        store: &mut S,
        owner: Address,
        delta: Self::Delta,
    ) -> Result<EvmStorage, StoreError>
    where
        S::Error: Debug;
}
