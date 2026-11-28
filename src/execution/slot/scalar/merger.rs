//! Merges scalar slots: accepted states' PosDelta and NegDelta changes into the shared totals.

use std::fmt::Debug;

use alloy_evm::revm::state::{EvmStorage, EvmStorageSlot};
use alloy_primitives::map::HashMap;
use alloy_primitives::{Address, U256};

use super::{KeyCodec, Property};
use crate::common::codec::CodecError;
use crate::store::SlotStore;
use crate::execution::slot::{ApplyDelta, database};
use crate::execution::workload::ID;
use crate::store::StoreError;

/// Merges scalars by adding each state's delta changes; conflict detection keeps them in bounds.
pub struct Applier;

/// Pending scalar changes of one owner, collected from accepted states until they are applied.
#[derive(Debug, Default)]
pub struct Delta {
    /// Per slot: the summed change (present − original) for PosDelta and NegDelta, the new value
    /// for the other properties.
    slots: HashMap<U256, U256>,
}

/// Returns whether a scalar slot property holds a delta magnitude.
fn is_delta(property: u8) -> bool {
    property == Property::PosDelta as u8 || property == Property::NegDelta as u8
}

impl ApplyDelta for Applier {
    type Delta = Delta;

    /// Adds the state's PosDelta and NegDelta changes; the other properties take the state's value
    /// where it changed them, as conflict detection lets only one state do so.
    fn merge_delta_from(delta: &mut Delta, _id: ID, slots: &EvmStorage) -> Result<(), CodecError> {
        for (&key, slot) in slots {
            let Some((_, property, _)) = KeyCodec::decode(key) else {
                continue;
            };
            if is_delta(property) {
                let total = delta.slots.entry(key).or_default();
                *total = total.wrapping_add(slot.present_value.wrapping_sub(slot.original_value));
            } else if slot.present_value != slot.original_value {
                delta.slots.insert(key, slot.present_value);
            }
        }
        Ok(())
    }

    /// Adds the summed PosDelta and NegDelta changes to the stored totals and writes the other
    /// changed properties.
    fn apply<S: SlotStore>(store: &mut S, owner: Address, delta: Delta) -> Result<(), StoreError>
    where
        S::Error: Debug,
    {
        for (key, value) in delta.slots {
            let value = match KeyCodec::decode(key) {
                Some((_, property, _)) if is_delta(property) => {
                    store.read(owner, key).map_err(database)?.wrapping_add(value)
                }
                _ => value,
            };
            store.write(owner, key, value).map_err(database)?;
        }
        Ok(())
    }
}

impl Applier {
    /// Returns `owner`'s scalars once `delta` is folded in at the persistent store: each Value
    /// takes its PosDelta and NegDelta, the other changed properties stay as they are, and no delta
    /// is kept.
    pub(crate) fn fold<S: SlotStore>(
        store: &mut S,
        owner: Address,
        delta: Delta,
    ) -> Result<EvmStorage, StoreError>
    where
        S::Error: Debug,
    {
        let slot = |value: U256| EvmStorageSlot::new(value, Default::default());
        let mut folded = EvmStorage::default();
        // Each scalar's net delta, by its Value key.
        let mut net = HashMap::<U256, U256>::default();
        for (key, value) in delta.slots {
            let value_key = KeyCodec::with_property(key, Property::Value as u8);
            match KeyCodec::decode(key) {
                Some((_, property, _)) if property == Property::PosDelta as u8 => {
                    let total = net.entry(value_key).or_default();
                    *total = total.wrapping_add(value);
                }
                Some((_, property, _)) if property == Property::NegDelta as u8 => {
                    let total = net.entry(value_key).or_default();
                    *total = total.wrapping_sub(value);
                }
                _ => {
                    folded.insert(key, slot(value));
                }
            }
        }
        for (key, change) in net {
            let value = match folded.get(&key) {
                Some(changed) => changed.present_value,
                None => store.read(owner, key).map_err(database)?,
            };
            folded.insert(key, slot(value.wrapping_add(change)));
        }
        Ok(folded)
    }
}
