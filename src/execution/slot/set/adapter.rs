//! Stores Sets in account-owned slots: the committed list plus a transaction's pending changes.

use alloy_primitives::{Address, U256};

use super::{SetKeyCodec, SetProperty};
use crate::collections::delta_set::DeltaSet;
use crate::common::codec::CodecError;
use crate::crdt::types::collections::SetDelta;
use crate::execution::evm::storage::ApiStore;

/// Reads and writes caller-owned Sets stored in an account's slots.
pub struct SetAdapter;

impl SetAdapter {
    /// Stages inserting `element`: a committed element only has its pending removal cancelled;
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
        let [added, removed] = [SetProperty::Added, SetProperty::Removed]
            .map(|property| SetKeyCodec::element(property, set, element, 0));
        if !storage.read(owner, added)?.is_zero() {
            return Ok(false);
        }

        if Self::successor(storage, owner, set, element)?.is_some() {
            if storage.read(owner, removed)?.is_zero() {
                return Ok(false);
            }
            // Cancel the pending removal, rewriting the removals log from its position.
            let log = SetKeyCodec::delta(SetProperty::PendingRemovals, set);
            let mut removals: Vec<U256> = log.decode(|key| storage.read(owner, key))?;
            let at = removals
                .binary_search(&element)
                .map_err(|_| CodecError::Invalid)?;
            removals.remove(at);
            log.encode_at::<_, _, U256, _>(&removals, at, |key, word| storage.write(owner, key, word))?;
            storage.write(owner, removed, U256::ZERO)?;
        } else {
            // Append to the additions log; the flag records the position for later cancellation.
            let log = SetKeyCodec::delta(SetProperty::DeltaAdded, set);
            let len = storage.read(owner, log.key(0)?)?;
            let at = u64::try_from(len / U256::from(32)).map_err(|_| CodecError::Invalid)?;
            storage.write(owner, log.key(at + 1)?, element)?;
            storage.write(owner, log.key(0)?, len + U256::from(32))?;
            storage.write(owner, added, U256::from(at + 1))?;
        }
        Ok(true)
    }

    /// Stages removing `element`: a pending addition is cancelled; a committed element joins the
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
        let [added, removed] = [SetProperty::Added, SetProperty::Removed]
            .map(|property| SetKeyCodec::element(property, set, element, 0));
        let position = storage.read(owner, added)?;
        if !position.is_zero() {
            // Cancel the pending addition: zero its word and mark its position as cancelled.
            let at = u64::try_from(position - U256::from(1)).map_err(|_| CodecError::Invalid)?;
            let log = SetKeyCodec::delta(SetProperty::DeltaAdded, set);
            storage.write(owner, log.key(at + 1)?, U256::ZERO)?;

            let cancelled = SetKeyCodec::delta(SetProperty::DeltaCancelled, set);
            let word = cancelled.key(at / 256 + 1)?;
            let bits = storage.read(owner, word)? | (U256::from(1) << (at % 256) as usize);
            storage.write(owner, word, bits)?;
            let len = storage
                .read(owner, cancelled.key(0)?)?
                .max(U256::from((at / 256 + 1) * 32));
            storage.write(owner, cancelled.key(0)?, len)?;
            storage.write(owner, added, U256::ZERO)?;
        } else if Self::successor(storage, owner, set, element)?.is_some()
            && storage.read(owner, removed)?.is_zero()
        {
            // Insert into the ascending removals log, rewriting it from the insertion point.
            let log = SetKeyCodec::delta(SetProperty::PendingRemovals, set);
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

    /// Returns whether `element` is live: pending added, or committed without a pending removal.
    pub fn contains<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
        element: U256,
    ) -> Result<bool, S::Error> {
        let [added, removed] = [SetProperty::Added, SetProperty::Removed]
            .map(|property| SetKeyCodec::element(property, set, element, 0));
        Ok(!storage.read(owner, added)?.is_zero()
            || (Self::successor(storage, owner, set, element)?.is_some()
                && storage.read(owner, removed)?.is_zero()))
    }

    /// Returns the live element count: committed, minus pending removals, plus uncancelled additions.
    pub fn length<S: ApiStore>(storage: &mut S, owner: Address, set: [u8; 6]) -> Result<U256, S::Error>
    where
        S::Error: From<CodecError>,
    {
        let committed = storage.read(owner, SetKeyCodec::length(set))?;
        let [added, removals, cancelled] = [
            SetProperty::DeltaAdded,
            SetProperty::PendingRemovals,
            SetProperty::DeltaCancelled,
        ]
        .map(|property| SetKeyCodec::delta(property, set));
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

    /// Returns the first live element: committed elements in list order, then pending additions.
    pub fn first<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
    ) -> Result<Option<U256>, S::Error>
    where
        S::Error: From<CodecError>,
    {
        if storage.read(owner, SetKeyCodec::length(set))?.is_zero() {
            return Self::pending_from(storage, owner, set, 0);
        }
        // The list is circular: the element after Tail is the first.
        let tail = storage.read(owner, SetKeyCodec::tail(set))?;
        let first = Self::successor(storage, owner, set, tail)?.ok_or(CodecError::Invalid)?;
        if storage.read(owner, SetKeyCodec::element(SetProperty::Removed, set, first, 0))?.is_zero() {
            return Ok(Some(first));
        }
        Self::next(storage, owner, set, first)
    }

    /// Returns the live element after `element`, skipping pending removals and cancelled additions;
    /// `None` at the end or if `element` is neither committed nor pending.
    pub fn next<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
        element: U256,
    ) -> Result<Option<U256>, S::Error>
    where
        S::Error: From<CodecError>,
    {
        let position = storage.read(owner, SetKeyCodec::element(SetProperty::Added, set, element, 0))?;
        if !position.is_zero() {
            let at = u64::try_from(position).map_err(|_| CodecError::Invalid)?;
            return Self::pending_from(storage, owner, set, at);
        }

        if Self::successor(storage, owner, set, element)?.is_none() {
            return Ok(None);
        }

        // Walk the committed list until a live element or the tail, then continue into pending additions.
        let tail = storage.read(owner, SetKeyCodec::tail(set))?;
        let mut current = element;
        loop {
            if current == tail {
                return Self::pending_from(storage, owner, set, 0);
            }
            let next = Self::successor(storage, owner, set, current)?.ok_or(CodecError::Invalid)?;
            if storage.read(owner, SetKeyCodec::element(SetProperty::Removed, set, next, 0))?.is_zero() {
                return Ok(Some(next));
            }
            current = next;
        }
    }

    /// Returns the committed element after `element`, or `None` if `element` isn't committed.
    /// A successor of 0 is stored under flag 1, since a zero word reads as empty.
    pub fn successor<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
        element: U256,
    ) -> Result<Option<U256>, S::Error> {
        let next = storage.read(owner, SetKeyCodec::element(SetProperty::Next, set, element, 0))?;
        if !next.is_zero() {
            return Ok(Some(next));
        }
        let zero = storage.read(owner, SetKeyCodec::element(SetProperty::Next, set, element, 1))?;
        Ok((!zero.is_zero()).then_some(U256::ZERO))
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
        let log = SetKeyCodec::delta(SetProperty::DeltaAdded, set);
        let count = u64::try_from(storage.read(owner, log.key(0)?)? / U256::from(32))
            .map_err(|_| CodecError::Invalid)?;
        for at in from..count {
            // A position is live while its element's flag still points to it.
            let element = storage.read(owner, log.key(at + 1)?)?;
            let added = SetKeyCodec::element(SetProperty::Added, set, element, 0);
            if storage.read(owner, added)? == U256::from(at + 1) {
                return Ok(Some(element));
            }
        }
        Ok(None)
    }

    /// Reads the transaction's pending delta for `set` from its three logs; empty logs read as empty.
    pub fn delta<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        set: [u8; 6],
    ) -> Result<SetDelta<U256>, S::Error>
    where
        S::Error: From<CodecError>,
    {
        let [added, cancelled, removals] = [
            SetProperty::DeltaAdded,
            SetProperty::DeltaCancelled,
            SetProperty::PendingRemovals,
        ]
        .map(|property| SetKeyCodec::delta(property, set));
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
