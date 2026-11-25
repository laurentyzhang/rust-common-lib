//! Storage format of Sets: the linked list, and its conversion to and from the execution format.

use std::fmt::Debug;

use alloy_primitives::{Address, U256};

use super::{KeyCodec, Property};
use crate::common::codec::CodecError;
use crate::execution::evm::storage::ApiStore;
use crate::execution::slot::{FormatConverter, database};
use crate::store::StoreError;

/// Converts a Set between its two formats: the storage format, a linked list persisted to the database,
/// and the execution format, an indexed array used from the set's first touch until block end.
pub struct Converter;

impl FormatConverter<[u8; 6]> for Converter {
    /// Converts the list into the execution format on the set's first touch in the block:
    /// Index holds the elements in list order and Position(x) each one's position + 1.
    fn to_execution_format<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
    ) -> Result<(), S::Error>
    where
        S::Error: From<CodecError>,
    {
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

    /// Relinks each converted set's live elements, in index order, into the list in place, then
    /// clears the execution format.
    fn to_storage_format<S: ApiStore>(
        store: &mut S,
        owner: Address,
        keys: &[U256],
    ) -> Result<(), StoreError>
    where
        S::Error: Debug,
    {
        for &key in keys {
            let Some((set, Property::Indexed, ..)) = KeyCodec::decode(key) else {
                continue;
            };
            if store.read(owner, key).map_err(database)?.is_zero() {
                continue;
            }
            let [index, holes] =
                [Property::Index, Property::Holes].map(|property| KeyCodec::delta(property, set));
            let count =
                u64::try_from(store.read(owner, index.key(0)?).map_err(database)? / U256::from(32))
                    .map_err(|_| CodecError::Invalid)?;
            let mut live = Vec::new();
            for at in 0..count {
                let entry = index.key(at + 1)?;
                if !store
                    .read(owner, holes.key(at / 256 + 1)?)
                    .map_err(database)?
                    .bit((at % 256) as usize)
                {
                    let element = store.read(owner, entry).map_err(database)?;
                    let position = KeyCodec::element(Property::Position, set, element);
                    store.write(owner, position, U256::ZERO).map_err(database)?;
                    live.push(element);
                }
                store.write(owner, entry, U256::ZERO).map_err(database)?;
            }

            // Only nodes whose successor changed are written; the last element's Node is empty.
            for (at, &element) in live.iter().enumerate() {
                let node = KeyCodec::element(Property::Node, set, element);
                let next = live.get(at + 1).copied().unwrap_or_default();
                if store.read(owner, node).map_err(database)? != next {
                    store.write(owner, node, next).map_err(database)?;
                }
            }
            let [head, tail] = [live.first(), live.last()].map(|end| end.copied().unwrap_or_default());
            store.write(owner, KeyCodec::head(set), head).map_err(database)?;
            store.write(owner, KeyCodec::tail(set), tail).map_err(database)?;

            let words =
                u64::try_from(store.read(owner, holes.key(0)?).map_err(database)? / U256::from(32))
                    .map_err(|_| CodecError::Invalid)?;
            for word in 0..=words {
                store
                    .write(owner, holes.key(word)?, U256::ZERO)
                    .map_err(database)?;
            }
            store
                .write(owner, index.key(0)?, U256::ZERO)
                .map_err(database)?;
            store.write(owner, key, U256::ZERO).map_err(database)?;
        }
        Ok(())
    }
}
