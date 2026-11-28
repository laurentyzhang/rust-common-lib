//! Converts Sets between the compact format, a linked list, and the indexed format, and applies
//! their pending changes at the persistent store.

use std::collections::BTreeMap;
use std::fmt::Debug;

use alloy_evm::revm::state::{EvmStorage, EvmStorageSlot};
use alloy_primitives::map::{HashMap, HashSet};
use alloy_primitives::{Address, U256};

use super::{Delta, KeyCodec, Property};
use crate::common::codec::CodecError;
use crate::store::SlotStore;
use crate::execution::slot::{FormatConverter, database};
use crate::store::StoreError;

/// Converts a Set between the compact format (a linked list) and the indexed format (an array).
pub struct Converter;

impl FormatConverter for Converter {
    type Delta = Delta;

    /// Expands the list of the Set `key` belongs to: Index holds the elements in list order,
    /// Position(x) each one's position + 1, and Length stays.
    fn expand<S: SlotStore>(storage: &mut S, owner: Address, key: U256) -> Result<(), S::Error>
    where
        S::Error: From<CodecError>,
    {
        let (set, ..) = KeyCodec::decode(key).ok_or(CodecError::Invalid)?;
        let indexed = KeyCodec::indexed(set);
        if !storage.read(owner, indexed)?.is_zero() {
            return Ok(());
        }
        let length = storage.read(owner, KeyCodec::length(set))?;
        let len = u64::try_from(length).map_err(|_| CodecError::Invalid)?;
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
        storage.write(owner, KeyCodec::length(set), length)?;
        storage.write(owner, indexed, U256::from(1))
    }

    /// Applies `delta` to each Set's committed elements and returns the changed slots of its list:
    /// the `Node`s whose successor changed, the last element's and a removed one's empty, then
    /// `Head`, `Tail` and `Length`. The indexed format never leaves the Set.
    fn compact<S: SlotStore>(
        store: &mut S,
        owner: Address,
        delta: Delta,
    ) -> Result<EvmStorage, StoreError>
    where
        S::Error: Debug,
    {
        let mut compacted = EvmStorage::default();
        for (set, base, new) in Self::commit(store, owner, delta)? {
            let next = |list: &[U256]| {
                list.windows(2)
                    .map(|pair| (pair[0], pair[1]))
                    .collect::<HashMap<_, _>>()
            };
            let ends = |list: &[U256]| {
                [list.first(), list.last()].map(|end| end.copied().unwrap_or_default())
            };
            let (before, after) = (next(&base), next(&new));
            let ([head, tail], [new_head, new_tail]) = (ends(&base), ends(&new));
            let mut changed = |key: U256, current: U256, value: U256| {
                if current != value {
                    compacted.insert(key, EvmStorageSlot::new_changed(current, value, Default::default()));
                }
            };
            for element in base.iter().chain(&new) {
                let [current, value] =
                    [&before, &after].map(|next| next.get(element).copied().unwrap_or_default());
                changed(KeyCodec::element(Property::Node, set, *element), current, value);
            }
            changed(KeyCodec::head(set), head, new_head);
            changed(KeyCodec::tail(set), tail, new_tail);
            changed(
                KeyCodec::length(set),
                U256::from(base.len()),
                U256::from(new.len()),
            );
        }
        Ok(compacted)
    }

    /// Applies `delta` to each Set's committed elements and returns the changed slots of its index,
    /// with no holes: from the first position that changed, each entry and its element's
    /// `Position`; a removed element's `Position` and the entries past the new end empty; then the
    /// count and `Length`.
    fn reindex<S: SlotStore>(
        store: &mut S,
        owner: Address,
        delta: Delta,
    ) -> Result<EvmStorage, StoreError>
    where
        S::Error: Debug,
    {
        let slot = |value: U256| EvmStorageSlot::new(value, Default::default());
        let mut reindexed = EvmStorage::default();
        for (set, base, new) in Self::commit(store, owner, delta)? {
            let index = KeyCodec::delta(Property::Index, set);
            let at = base.iter().zip(&new).take_while(|(old, new)| old == new).count();
            for position in at..base.len().max(new.len()) {
                let element = new.get(position).copied().unwrap_or_default();
                reindexed.insert(index.key(position as u64 + 1)?, slot(element));
            }
            let live: HashSet<U256> = new.iter().copied().collect();
            for &element in base[at..].iter().filter(|element| !live.contains(*element)) {
                reindexed.insert(KeyCodec::element(Property::Position, set, element), slot(U256::ZERO));
            }
            for (position, &element) in new.iter().enumerate().skip(at) {
                let key = KeyCodec::element(Property::Position, set, element);
                reindexed.insert(key, slot(U256::from(position + 1)));
            }
            reindexed.insert(index.key(0)?, slot(U256::from(new.len() * 32)));
            reindexed.insert(KeyCodec::length(set), slot(U256::from(new.len())));
        }
        Ok(reindexed)
    }
}

impl Converter {
    /// Returns each Set `delta` touches, with its committed elements in `store` and its elements
    /// once `delta` is applied: the removals leave, then the additions follow in (block,
    /// generation, sequence, job) order, each element once.
    fn commit<S: SlotStore>(
        store: &mut S,
        owner: Address,
        mut delta: Delta,
    ) -> Result<Vec<([u8; 6], Vec<U256>, Vec<U256>)>, StoreError>
    where
        S::Error: Debug,
    {
        let sets: HashSet<[u8; 6]> = delta
            .additions
            .keys()
            .chain(delta.removals.keys())
            .copied()
            .collect();
        let mut committed = Vec::new();
        for set in sets {
            let base = Self::elements(store, owner, set)?;
            let removals = delta.removals.remove(&set).unwrap_or_default();
            let mut seen = HashSet::<U256>::default();
            let new = base
                .iter()
                .copied()
                .filter(|element| !removals.contains(element))
                .chain(
                    delta
                        .additions
                        .remove(&set)
                        .into_iter()
                        .flat_map(BTreeMap::into_values)
                        .flatten(),
                )
                .filter(|&element| seen.insert(element))
                .collect();
            committed.push((set, base, new));
        }
        Ok(committed)
    }

    /// Returns `owner`'s committed elements of `set` in order: from the index once expanded, else
    /// by walking the list from `Head` for `Length` elements.
    fn elements<S: SlotStore>(
        store: &mut S,
        owner: Address,
        set: [u8; 6],
    ) -> Result<Vec<U256>, StoreError>
    where
        S::Error: Debug,
    {
        let mut read = |key: U256| store.read(owner, key).map_err(database);
        let len = u64::try_from(read(KeyCodec::length(set))?).map_err(|_| CodecError::Invalid)?;
        if !read(KeyCodec::indexed(set))?.is_zero() {
            let index = KeyCodec::delta(Property::Index, set);
            return (1..=len).map(|at| read(index.key(at)?)).collect();
        }
        let mut current = read(KeyCodec::head(set))?;
        let mut elements = Vec::new();
        for at in 0..len {
            if at > 0 {
                current = read(KeyCodec::element(Property::Node, set, current))?;
            }
            elements.push(current);
        }
        Ok(elements)
    }
}
