//! Merges Set slots: each sequence's pending changes into the execution format.

use std::collections::BTreeSet;
use std::fmt::Debug;

use alloy_evm::revm::state::EvmStorage;
use alloy_primitives::{Address, U256};

use super::{KeyCodec, Property};
use crate::common::codec::CodecError;
use crate::execution::evm::storage::ApiStore;
use crate::execution::slot::{DeltaMerger, database};
use crate::store::StoreError;

/// Merges Sets by union; conflict detection leaves only independent deltas over one base.
pub struct Merger;

impl DeltaMerger for Merger {
    /// Adopts the sequence's conversion if the block has none yet, appends its uncancelled
    /// additions to the index and turns its removals into holes; Length keeps the live count.
    fn merge<S: ApiStore>(store: &mut S, owner: Address, slots: &EvmStorage) -> Result<(), StoreError>
    where
        S::Error: Debug,
    {
        let get = |key: U256| slots.get(&key).map_or(U256::ZERO, |slot| slot.present_value);
        let sets: BTreeSet<[u8; 6]> = slots
            .keys()
            .filter_map(|&key| KeyCodec::decode(key))
            .map(|(set, ..)| set)
            .collect();
        for set in sets {
            // Parallel sequences convert the same list identically, so the first conversion is kept.
            let indexed = KeyCodec::indexed(set);
            if store.read(owner, indexed).map_err(database)?.is_zero() && !get(indexed).is_zero() {
                for (&key, slot) in slots {
                    if let Some((id, Property::Index | Property::Position | Property::Indexed, ..)) =
                        KeyCodec::decode(key)
                        && id == set
                    {
                        store.write(owner, key, slot.present_value).map_err(database)?;
                    }
                }
            }

            let [added, cancelled, removals] = [
                Property::DeltaAdded,
                Property::DeltaCancelled,
                Property::PendingRemovals,
            ]
            .map(|property| KeyCodec::delta(property, set));
            let added: Vec<U256> = added.decode(|key| Ok::<_, StoreError>(get(key)))?;
            let cancelled: Vec<U256> = cancelled.decode(|key| Ok::<_, StoreError>(get(key)))?;
            let removals: Vec<U256> = removals.decode(|key| Ok::<_, StoreError>(get(key)))?;
            if added.is_empty() && removals.is_empty() {
                continue;
            }
            let length = KeyCodec::length(set);
            let mut live = store.read(owner, length).map_err(database)?;

            // Append the uncancelled additions in order.
            let index = KeyCodec::delta(Property::Index, set);
            let mut count = u64::try_from(store.read(owner, index.key(0)?).map_err(database)? / U256::from(32))
                .map_err(|_| CodecError::Invalid)?;
            for (at, element) in added.into_iter().enumerate() {
                if cancelled.get(at / 256).is_some_and(|bits| bits.bit(at % 256)) {
                    continue;
                }
                let position = KeyCodec::element(Property::Position, set, element);
                store.write(owner, index.key(count + 1)?, element).map_err(database)?;
                store.write(owner, position, U256::from(count + 1)).map_err(database)?;
                count += 1;
                live += U256::from(1);
            }
            store.write(owner, index.key(0)?, U256::from(count * 32)).map_err(database)?;

            // A removal leaves a hole: zero its entry, mark it in Holes, clear its Position and Node.
            let holes = KeyCodec::delta(Property::Holes, set);
            let mut holes_len = store.read(owner, holes.key(0)?).map_err(database)?;
            for element in removals {
                let position = KeyCodec::element(Property::Position, set, element);
                let at = u64::try_from(store.read(owner, position).map_err(database)?)
                    .ok()
                    .and_then(|position| position.checked_sub(1))
                    .ok_or(CodecError::Invalid)?;
                store.write(owner, index.key(at + 1)?, U256::ZERO).map_err(database)?;
                let word = holes.key(at / 256 + 1)?;
                let bits = store.read(owner, word).map_err(database)? | (U256::from(1) << (at % 256) as usize);
                store.write(owner, word, bits).map_err(database)?;
                holes_len = holes_len.max(U256::from((at / 256 + 1) * 32));
                store.write(owner, position, U256::ZERO).map_err(database)?;
                let node = KeyCodec::element(Property::Node, set, element);
                store.write(owner, node, U256::ZERO).map_err(database)?;
                live -= U256::from(1);
            }
            store.write(owner, holes.key(0)?, holes_len).map_err(database)?;
            store.write(owner, length, live).map_err(database)?;
        }
        Ok(())
    }
}
