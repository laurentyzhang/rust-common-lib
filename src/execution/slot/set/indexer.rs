//! Execution format of Sets: contract reads and writes over the index plus the pending changes.

use alloy_primitives::{Address, U256};

use super::{KeyCodec, Property};
use crate::collections::delta_set::DeltaSet;
use crate::common::codec::CodecError;
use crate::crdt::types::collections::SetDelta;
use crate::execution::evm::api_store::ApiStore;

/// Reads and writes caller-owned Sets in the execution format; `Converter` converts them in and out.
pub struct Indexer;

impl Indexer {
    /// Stages inserting `element`: an indexed element only has its pending removal cancelled;
    /// otherwise it is appended to the pending additions. Returns whether the pending delta changed.
    pub fn insert<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
        element: U256,
    ) -> Result<bool, S::Error>
    where
        S::Error: From<CodecError>,
    {
        let [added, removed] = [Property::Added, Property::Removed]
            .map(|property| KeyCodec::element(property, set, element));
        if !storage.read(owner, added)?.is_zero() {
            return Ok(false);
        }

        if Self::committed(storage, owner, set, element)? {
            if storage.read(owner, removed)?.is_zero() {
                return Ok(false);
            }
            // Cancel the pending removal, rewriting the removals log from its position.
            let log = KeyCodec::delta(Property::PendingRemovals, set);
            let mut removals: Vec<U256> = log.decode(|key| storage.read(owner, key))?;
            let at = removals
                .binary_search(&element)
                .map_err(|_| CodecError::Invalid)?;
            removals.remove(at);
            log.encode_at::<_, _, U256, _>(&removals, at, |key, word| storage.write(owner, key, word))?;
            storage.write(owner, removed, U256::ZERO)?;
        } else {
            // Append to the additions log; the flag records the position for later cancellation.
            let log = KeyCodec::delta(Property::DeltaAdded, set);
            let len = storage.read(owner, log.key(0)?)?;
            let at = u64::try_from(len / U256::from(32)).map_err(|_| CodecError::Invalid)?;
            storage.write(owner, log.key(at + 1)?, element)?;
            storage.write(owner, log.key(0)?, len + U256::from(32))?;
            storage.write(owner, added, U256::from(at + 1))?;
        }
        Ok(true)
    }

    /// Stages removing `element`: a pending addition is cancelled; an indexed element joins the
    /// pending removals. Returns whether the pending delta changed.
    pub fn remove<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
        element: U256,
    ) -> Result<bool, S::Error>
    where
        S::Error: From<CodecError>,
    {
        let [added, removed] = [Property::Added, Property::Removed]
            .map(|property| KeyCodec::element(property, set, element));
        let position = storage.read(owner, added)?;
        if !position.is_zero() {
            // Cancel the pending addition: zero its word and mark its position as cancelled.
            let at = u64::try_from(position - U256::from(1)).map_err(|_| CodecError::Invalid)?;
            let log = KeyCodec::delta(Property::DeltaAdded, set);
            storage.write(owner, log.key(at + 1)?, U256::ZERO)?;

            let cancelled = KeyCodec::delta(Property::DeltaCancelled, set);
            let word = cancelled.key(at / 256 + 1)?;
            let bits = storage.read(owner, word)? | (U256::from(1) << (at % 256) as usize);
            storage.write(owner, word, bits)?;
            let len = storage
                .read(owner, cancelled.key(0)?)?
                .max(U256::from((at / 256 + 1) * 32));
            storage.write(owner, cancelled.key(0)?, len)?;
            storage.write(owner, added, U256::ZERO)?;
        } else if Self::committed(storage, owner, set, element)?
            && storage.read(owner, removed)?.is_zero()
        {
            // Insert into the ascending removals log, rewriting it from the insertion point.
            let log = KeyCodec::delta(Property::PendingRemovals, set);
            let mut removals: Vec<U256> = log.decode(|key| storage.read(owner, key))?;
            let at = match removals.binary_search(&element) {
                Ok(_) => return Err(CodecError::Invalid.into()),
                Err(at) => at,
            };
            removals.insert(at, element);
            log.encode_at::<_, _, U256, _>(&removals, at, |key, word| storage.write(owner, key, word))?;
            storage.write(owner, removed, U256::from(1))?;
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// Returns whether `element` is live: pending added, or indexed without a pending removal.
    pub fn contains<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
        element: U256,
    ) -> Result<bool, S::Error> {
        let [added, removed] = [Property::Added, Property::Removed]
            .map(|property| KeyCodec::element(property, set, element));
        Ok(!storage.read(owner, added)?.is_zero()
            || (Self::committed(storage, owner, set, element)?
                && storage.read(owner, removed)?.is_zero()))
    }

    /// Returns whether `element` is in the index, i.e. in the set before the pending changes.
    fn committed<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
        element: U256,
    ) -> Result<bool, S::Error> {
        let position = KeyCodec::element(Property::Position, set, element);
        Ok(!storage.read(owner, position)?.is_zero())
    }

    /// Returns the first index element at or after position `from` that is neither a hole
    /// nor pending removal.
    fn indexed_from<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
        from: u64,
    ) -> Result<Option<U256>, S::Error>
    where
        S::Error: From<CodecError>,
    {
        let [index, holes] =
            [Property::Index, Property::Holes].map(|property| KeyCodec::delta(property, set));
        let count = u64::try_from(storage.read(owner, index.key(0)?)? / U256::from(32))
            .map_err(|_| CodecError::Invalid)?;
        for at in from..count {
            if storage.read(owner, holes.key(at / 256 + 1)?)?.bit((at % 256) as usize) {
                continue;
            }
            let element = storage.read(owner, index.key(at + 1)?)?;
            if storage.read(owner, KeyCodec::element(Property::Removed, set, element))?.is_zero() {
                return Ok(Some(element));
            }
        }
        Ok(None)
    }

    /// Returns the live element count: Length, minus pending removals, plus uncancelled additions.
    pub fn length<S: ApiStore>(storage: &mut S, owner: Address, set: [u8; 6]) -> Result<U256, S::Error>
    where
        S::Error: From<CodecError>,
    {
        let committed = storage.read(owner, KeyCodec::length(set))?;
        let [added, removals, cancelled] = [
            Property::DeltaAdded,
            Property::PendingRemovals,
            Property::DeltaCancelled,
        ]
        .map(|property| KeyCodec::delta(property, set));
        let added = storage.read(owner, added.key(0)?)? / U256::from(32);
        let removals = storage.read(owner, removals.key(0)?)? / U256::from(32);

        // Count the cancelled positions in the bitmap words.
        let words = u64::try_from(storage.read(owner, cancelled.key(0)?)? / U256::from(32))
            .map_err(|_| CodecError::Invalid)?;
        let mut holes = 0;
        for word in 1..=words {
            holes += storage.read(owner, cancelled.key(word)?)?.count_ones();
        }
        Ok(committed + added - removals - U256::from(holes))
    }

    /// Returns the first live element: index order, then pending additions.
    pub fn first<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
    ) -> Result<Option<U256>, S::Error>
    where
        S::Error: From<CodecError>,
    {
        match Self::indexed_from(storage, owner, set, 0)? {
            Some(element) => Ok(Some(element)),
            None => Self::pending_from(storage, owner, set, 0),
        }
    }

    /// Returns the live element after `element`, skipping pending removals and cancelled additions;
    /// `None` at the end or if `element` is neither indexed nor pending.
    pub fn next<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
        element: U256,
    ) -> Result<Option<U256>, S::Error>
    where
        S::Error: From<CodecError>,
    {
        let position = storage.read(owner, KeyCodec::element(Property::Added, set, element))?;
        if !position.is_zero() {
            let at = u64::try_from(position).map_err(|_| CodecError::Invalid)?;
            return Self::pending_from(storage, owner, set, at);
        }

        // Continue after the element's index position, then into pending additions.
        let position = storage.read(owner, KeyCodec::element(Property::Position, set, element))?;
        if position.is_zero() {
            return Ok(None);
        }
        let from = u64::try_from(position).map_err(|_| CodecError::Invalid)?;
        match Self::indexed_from(storage, owner, set, from)? {
            Some(next) => Ok(Some(next)),
            None => Self::pending_from(storage, owner, set, 0),
        }
    }

    /// Returns the first uncancelled pending addition at or after position `from`.
    fn pending_from<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
        from: u64,
    ) -> Result<Option<U256>, S::Error>
    where
        S::Error: From<CodecError>,
    {
        let log = KeyCodec::delta(Property::DeltaAdded, set);
        let count = u64::try_from(storage.read(owner, log.key(0)?)? / U256::from(32))
            .map_err(|_| CodecError::Invalid)?;
        for at in from..count {
            // A position is live while its element's flag still points to it.
            let element = storage.read(owner, log.key(at + 1)?)?;
            let added = KeyCodec::element(Property::Added, set, element);
            if storage.read(owner, added)? == U256::from(at + 1) {
                return Ok(Some(element));
            }
        }
        Ok(None)
    }

    /// Reads the pending changes of `set` as a delta from its three logs; empty logs read as empty.
    pub fn delta<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
    ) -> Result<SetDelta<U256>, S::Error>
    where
        S::Error: From<CodecError>,
    {
        let [added, cancelled, removals] = [
            Property::DeltaAdded,
            Property::DeltaCancelled,
            Property::PendingRemovals,
        ]
        .map(|property| KeyCodec::delta(property, set));
        let added: Vec<U256> = added.decode(|key| storage.read(owner, key))?;
        let cancelled: Vec<U256> = cancelled.decode(|key| storage.read(owner, key))?;
        let removals: Vec<U256> = removals.decode(|key| storage.read(owner, key))?;

        let slots = added
            .into_iter()
            .enumerate()
            .map(|(i, element)| {
                let hole = cancelled.get(i / 256).is_some_and(|bits| bits.bit(i % 256));
                (!hole).then_some(element)
            })
            .collect();
        Ok(SetDelta {
            added: DeltaSet::try_from_slots(slots).ok_or(CodecError::Invalid)?,
            removed: removals.into_iter().collect(),
        })
    }
}
