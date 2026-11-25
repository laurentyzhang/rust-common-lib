//! Merges scalar slots: each state's PosDelta and NegDelta changes into the shared totals.

use std::fmt::Debug;

use alloy_evm::revm::state::EvmStorage;
use alloy_primitives::{Address, U256};

use super::{KeyCodec, Property};
use crate::execution::evm::storage::ApiStore;
use crate::execution::slot::{DeltaMerger, database};
use crate::store::StoreError;

/// Merges scalars by adding each state's delta changes; conflict detection keeps them in bounds.
pub struct Merger;

impl DeltaMerger for Merger {
    /// Adds each PosDelta and NegDelta change (present − original) to the stored total; the other
    /// scalar slots take the incoming value.
    fn merge<S: ApiStore>(store: &mut S, owner: Address, slots: &EvmStorage) -> Result<(), StoreError>
    where
        S::Error: Debug,
    {
        for (&key, slot) in slots {
            let value = match KeyCodec::decode(key) {
                Some((_, property, _))
                    if property == Property::PosDelta as u8 || property == Property::NegDelta as u8 =>
                {
                    let change: U256 = slot.present_value.wrapping_sub(slot.original_value);
                    store.read(owner, key).map_err(database)?.wrapping_add(change)
                }
                _ => slot.present_value,
            };
            store.write(owner, key, value).map_err(database)?;
        }
        Ok(())
    }
}
