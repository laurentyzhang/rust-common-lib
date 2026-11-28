//! Converts Sets between the compact format, a linked list, and the indexed format.

use std::collections::BTreeSet;
use std::fmt::Debug;

use alloy_evm::revm::state::{EvmStorage, EvmStorageSlot};
use alloy_primitives::{Address, U256};

use super::{KeyCodec, Property};
use crate::common::codec::CodecError;
use crate::store::SlotStore;
use crate::execution::slot::{FormatConverter, database};
use crate::store::StoreError;

/// Converts a Set between the compact format (a linked list) and the indexed format (an array).
pub struct Converter;

impl FormatConverter for Converter {
    /// Expands the list of the Set `key` belongs to: Index holds the elements in list order,
    /// Position(x) each one's position + 1.
    fn expand<S: SlotStore>(storage: &mut S, owner: Address, key: U256) -> Result<(), S::Error>
    where
        S::Error: From<CodecError>,
    {
        let (set, ..) = KeyCodec::decode(key).ok_or(CodecError::Invalid)?;
        let indexed = KeyCodec::indexed(set);
        if !storage.read(owner, indexed)?.is_zero() {
            return Ok(());
        }
        let len = u64::try_from(storage.read(owner, KeyCodec::length(set))?)
            .map_err(|_| CodecError::Invalid)?;
        // Walk Length elements from Head; an empty Node before the last element reads as 0.
        let mut current = storage.read(owner, KeyCodec::head(set))?;
        let mut elements = Vec::new();
        for at in 0..len {
            if at > 0 {
                current = storage.read(owner, KeyCodec::element(Property::Node, set, current))?;
            }
            let position = KeyCodec::element(Property::Position, set, current);
            storage.write(owner, position, U256::from(at + 1))?;
            elements.push(current);
        }
        KeyCodec::delta(Property::Index, set)
            .encode(&elements, |key, word| storage.write(owner, key, word))?;
        storage.write(owner, indexed, U256::from(1))
    }

    /// Returns the Sets among `slots` in the compact format: their compact-format slots as they are,
    /// and each expanded Set's live elements relinked, in index order, into the list. The indexed
    /// format never leaves the Set.
    fn compact<S: SlotStore>(
        store: &mut S,
        owner: Address,
        slots: &EvmStorage,
    ) -> Result<EvmStorage, StoreError>
    where
        S::Error: Debug,
    {
        // The current value: the change in `slots`, else the store's.
        let read = |store: &mut S, key: U256| match slots.get(&key) {
            Some(slot) => Ok(slot.present_value),
            None => store.read(owner, key).map_err(database),
        };
        let mut compacted = EvmStorage::default();
        let mut sets = BTreeSet::new();
        for (&key, slot) in slots {
            let Some((set, property, _)) = KeyCodec::decode(key) else {
                continue;
            };
            sets.insert(set);
            if !matches!(
                property,
                Property::Index | Property::Position | Property::Holes | Property::Indexed
            ) {
                compacted.insert(key, slot.clone());
            }
        }
        for set in sets {
            if read(store, KeyCodec::indexed(set))?.is_zero() {
                continue;
            }
            let [index, holes] =
                [Property::Index, Property::Holes].map(|property| KeyCodec::delta(property, set));
            let count = u64::try_from(read(store, index.key(0)?)? / U256::from(32))
                .map_err(|_| CodecError::Invalid)?;
            let mut live = Vec::new();
            for at in 0..count {
                if !read(store, holes.key(at / 256 + 1)?)?.bit((at % 256) as usize) {
                    live.push(read(store, index.key(at + 1)?)?);
                }
            }

            // Only nodes whose successor changed are written; the last element's Node is empty.
            for (at, &element) in live.iter().enumerate() {
                let node = KeyCodec::element(Property::Node, set, element);
                let next = live.get(at + 1).copied().unwrap_or_default();
                let current = read(store, node)?;
                if current != next {
                    compacted.insert(node, EvmStorageSlot::new_changed(current, next, Default::default()));
                }
            }
            let [head, tail] = [live.first(), live.last()].map(|end| end.copied().unwrap_or_default());
            for (key, value) in [(KeyCodec::head(set), head), (KeyCodec::tail(set), tail)] {
                let current = read(store, key)?;
                compacted.insert(key, EvmStorageSlot::new_changed(current, value, Default::default()));
            }
        }
        Ok(compacted)
    }
}
