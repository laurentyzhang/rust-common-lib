//! Stores Set elements in account-owned slots and folds pending flags into committed indexes.

use alloy_primitives::{Address, U256};

use super::{SetKeyCodec, SetProperty};
use crate::collections::delta_set::DeltaSet;
use crate::crdt::{Crdt, Set, state::DeltaOp};
use crate::execution::evm::storage::ApiStore;
use crate::store::StoreError;

/// Reads, writes, and commits the caller-owned Set stored in an account's slots.
pub struct SetAdapter;

impl SetAdapter {
    /// Stores an element's byte length and 32-byte chunks under its ID.
    pub fn write_element<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        id: [u8; 24],
        element: &[u8],
    ) -> Result<(), S::Error> {
        storage.write(
            owner,
            SetKeyCodec::encode(SetProperty::Size, id),
            U256::from(element.len()),
        )?;
        for (j, chunk) in element.chunks(32).enumerate() {
            let mut word = [0; 32];
            word[..chunk.len()].copy_from_slice(chunk);
            storage.write(
                owner,
                SetKeyCodec::chunk(id, j as u64),
                U256::from_be_bytes(word),
            )?;
        }
        Ok(())
    }

    /// Reads an element's bytes back from its length and 32-byte chunks.
    pub fn read_element<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        id: [u8; 24],
    ) -> Result<Vec<u8>, S::Error> {
        let size = storage
            .read(owner, SetKeyCodec::encode(SetProperty::Size, id))?
            .saturating_to::<usize>();
        let mut element = Vec::with_capacity(size);
        for j in 0..size.div_ceil(32) {
            let word = storage
                .read(owner, SetKeyCodec::chunk(id, j as u64))?
                .to_be_bytes::<32>();
            element.extend_from_slice(&word[..(size - j * 32).min(32)]);
        }
        Ok(element)
    }

    /// Sets an element's pending flag and appends its ID to the owner's pending list.
    /// Other flags are not read, so a parallel flag on the same element is never overwritten.
    pub fn flag<S: ApiStore>(
        storage: &mut S,
        owner: Address,
        id: [u8; 24],
        property: SetProperty,
    ) -> Result<(), S::Error> {
        let count = SetKeyCodec::encode(SetProperty::PendingLength, [0; 24]);
        let k = storage.read(owner, count)?;
        storage.write(
            owner,
            SetKeyCodec::pending(k.saturating_to()),
            U256::from_be_slice(&id),
        )?;
        storage.write(owner, count, k + U256::from(1))?;
        storage.write(owner, SetKeyCodec::encode(property, id), U256::from(1))
    }

    /// Folds the owner's pending list, so pending elements get indexes in first-flag order.
    pub fn fold_pending<S>(storage: &mut S, owner: Address) -> Result<(), StoreError>
    where
        S: ApiStore,
        S::Error: std::fmt::Display,
    {
        let error = |error: S::Error| StoreError::Database(error.to_string());
        let count = storage
            .read(
                owner,
                SetKeyCodec::encode(SetProperty::PendingLength, [0; 24]),
            )
            .map_err(error)?
            .saturating_to::<u64>();
        if count == 0 {
            return Ok(());
        }

        // Collect each listed element's live flags once, keeping list order.
        let mut seen = std::collections::HashSet::new();
        let mut flags = Vec::new();
        for k in 0..count {
            let word = storage
                .read(owner, SetKeyCodec::pending(k))
                .map_err(error)?;
            let id: [u8; 24] = word.to_be_bytes::<32>()[8..].try_into().unwrap();
            if !seen.insert(id) {
                continue;
            }
            for property in [SetProperty::Removed, SetProperty::Added] {
                let flag = SetKeyCodec::encode(property, id);
                if !storage.read(owner, flag).map_err(error)?.is_zero() {
                    flags.push((property, id));
                }
            }
        }
        Self::fold(storage, owner, &flags)
    }

    /// Applies pending Added/Removed flags to the owner's committed indexes through `crdt::Set`
    /// (removes first, then adds), writes changed Index/Member slots and Length, and clears the
    /// flags and the pending list.
    pub fn fold<S>(
        storage: &mut S,
        owner: Address,
        flags: &[(SetProperty, [u8; 24])],
    ) -> Result<(), StoreError>
    where
        S: ApiStore,
        S::Error: std::fmt::Display,
    {
        let error = |error: S::Error| StoreError::Database(error.to_string());
        let length = SetKeyCodec::encode(SetProperty::Length, [0; 24]);

        // Load committed index slots; a zero word is a removed slot.
        let mut slots = Vec::new();
        for i in 0..storage
            .read(owner, length)
            .map_err(error)?
            .saturating_to::<u64>()
        {
            let word = storage.read(owner, SetKeyCodec::index(i)).map_err(error)?;
            slots
                .push((!word.is_zero()).then(|| word.to_be_bytes::<32>()[8..].try_into().unwrap()));
        }

        let entries = DeltaSet::try_from_slots(slots.clone())
            .ok_or_else(|| StoreError::Codec(format!("duplicate set element for {owner}")))?;
        let mut set = Set {
            entries,
            delta: None,
        };
        let ops: Vec<_> = flags
            .iter()
            .map(|&(property, id)| match property {
                SetProperty::Removed => DeltaOp::Sub(id),
                _ => DeltaOp::Add(id),
            })
            .collect();
        set.add_delta(&ops)?;
        set.apply_delta();
        let updated = set.entries.slots();

        // Clear removed elements first, so a removed-and-added element keeps its new index.
        for (i, id) in slots.iter().enumerate() {
            if let Some(id) = id
                && updated[i].is_none()
            {
                storage
                    .write(
                        owner,
                        SetKeyCodec::encode(SetProperty::Member, *id),
                        U256::ZERO,
                    )
                    .map_err(error)?;
                storage
                    .write(owner, SetKeyCodec::index(i as u64), U256::ZERO)
                    .map_err(error)?;
            }
        }

        // Record appended elements at their new indexes.
        for (i, id) in updated.iter().enumerate().skip(slots.len()) {
            if let Some(id) = id {
                storage
                    .write(owner, SetKeyCodec::index(i as u64), U256::from_be_slice(id))
                    .map_err(error)?;
                storage
                    .write(
                        owner,
                        SetKeyCodec::encode(SetProperty::Member, *id),
                        U256::from(i + 1),
                    )
                    .map_err(error)?;
            }
        }

        storage
            .write(owner, length, U256::from(updated.len()))
            .map_err(error)?;
        for &(property, id) in flags {
            storage
                .write(owner, SetKeyCodec::encode(property, id), U256::ZERO)
                .map_err(error)?;
        }
        storage
            .write(
                owner,
                SetKeyCodec::encode(SetProperty::PendingLength, [0; 24]),
                U256::ZERO,
            )
            .map_err(error)?;
        Ok(())
    }
}
