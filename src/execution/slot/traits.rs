//! Traits implemented by the CRDT namespaces stored in EVM slots.

use std::fmt::Debug;

use alloy_evm::revm::state::EvmStorage;
use alloy_primitives::{Address, U256};

use crate::common::codec::CodecError;
use crate::execution::evm::api_store::ApiStore;
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
    fn apply<S: ApiStore>(store: &mut S, owner: Address, delta: Self::Delta) -> Result<(), StoreError>
    where
        S::Error: Debug;
}

/// Converts one CRDT namespace between its compact format and its indexed format.
pub trait FormatConverter<Id> {
    /// Expands `owner`'s value `id` into the indexed format; does nothing once expanded.
    fn expand<S: ApiStore>(
        store: &mut S,
        owner: Address,
        id: Id,
    ) -> Result<(), S::Error>
    where
        S::Error: From<CodecError>;

    /// Compacts the indexed values among `owner`'s `keys` back into the compact format.
    fn compact<S: ApiStore>(
        store: &mut S,
        owner: Address,
        keys: &[U256],
    ) -> Result<(), StoreError>
    where
        S::Error: Debug;
}
