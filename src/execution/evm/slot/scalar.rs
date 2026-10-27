//! Maps CRDT scalars to account-owned storage slots and encodes their pending deltas.

use alloy_primitives::{Address, U256};

use super::SlotStorage;
use crate::crdt;
use crate::execution::evm::{ApiAddress, CallHookError, SlotCodec};

/// Scalar field IDs encoded in byte 7 of a slot key.
#[derive(strum_macros::FromRepr)]
#[repr(u8)]
pub enum Field {
    Min = 0,
    Max = 1,
    Value = 2,
    Delta = 3,
}

/// Holds the four slot keys for one scalar instance, rather than their stored values.
/// The owner address is supplied separately when accessing storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalarSlot {
    pub min: U256,
    pub max: U256,
    pub value: U256,
    pub delta: U256,
}

// Read individual fields using any recognized field key of the scalar instance.
// Readers return None for invalid keys or backend read failures; missing slots read as zero.
impl ScalarSlot {
    /// Validates the scalar API and field ID, then derives all four field keys.
    fn parse(key: U256) -> Option<Self> {
        let (api, field, _) = SlotCodec::decode(key)?;
        if api != ApiAddress::Scalar || Field::from_repr(field).is_none() {
            return None;
        }
        Some(Self::new(key))
    }

    /// Reads the lower bound for the scalar identified by `key` under `owner`.
    pub fn read_min<S>(storage: &mut S, owner: Address, key: U256) -> Option<U256>
    where
        S: SlotStorage,
    {
        let slots = Self::parse(key)?;
        storage.read(owner, slots.min).ok()
    }

    /// Reads the upper bound for the scalar identified by `key` under `owner`.
    pub fn read_max<S>(storage: &mut S, owner: Address, key: U256) -> Option<U256>
    where
        S: SlotStorage,
    {
        let slots = Self::parse(key)?;
        storage.read(owner, slots.max).ok()
    }

    /// Reads the base value without applying the pending delta.
    pub fn read_value<S>(storage: &mut S, owner: Address, key: U256) -> Option<U256>
    where
        S: SlotStorage,
    {
        let slots = Self::parse(key)?;
        storage.read(owner, slots.value).ok()
    }

    /// Reads the raw modular delta encoding, without converting it to `DeltaOp`.
    pub fn read_delta<S>(storage: &mut S, owner: Address, key: U256) -> Option<U256>
    where
        S: SlotStorage,
    {
        let slots = Self::parse(key)?;
        storage.read(owner, slots.delta).ok()
    }
}

// Construct a slot layout and transfer whole scalar states through a backend.
impl ScalarSlot {
    /// Derives all four field keys while preserving the instance key and prefix.
    /// The caller must validate the input key before calling this constructor.
    pub fn new(key: U256) -> Self {
        Self {
            min: SlotCodec::with_field(key, Field::Min as u8),
            max: SlotCodec::with_field(key, Field::Max as u8),
            value: SlotCodec::with_field(key, Field::Value as u8),
            delta: SlotCodec::with_field(key, Field::Delta as u8),
        }
    }

    /// Reads all four fields and reconstructs the scalar's pending arithmetic operation.
    /// Returns the scalar and a copy of this layout without applying the delta.
    pub fn load<S>(
        &self,
        storage: &mut S,
        owner: Address,
    ) -> Result<(crdt::Scalar<U256>, Self), CallHookError>
    where
        S: SlotStorage,
        S::Error: std::fmt::Display,
    {
        let mut state = crdt::Scalar::<U256>::default();
        let mut delta = U256::ZERO;
        for (slot, value) in [
            (self.min, &mut state.limits.0),
            (self.max, &mut state.limits.1),
            (self.value, &mut state.value),
            (self.delta, &mut delta),
        ] {
            *value = storage
                .read(owner, slot)
                .map_err(|error| CallHookError::Handler(error.to_string()))?;
        }

        // Recover the projected value modulo 2^256, then use the base to recover direction.
        // A zero encoding represents no pending delta.
        state.delta = (!delta.is_zero()).then(|| {
            <U256 as crdt::Type>::delta_between(state.value, state.value.wrapping_add(delta))
        });
        Ok((state, *self))
    }

    /// Writes bounds, base value, and encoded pending delta to the mapped slots.
    /// Checks pending arithmetic before writing and leaves the supplied scalar unchanged.
    pub fn store<S>(
        &self,
        storage: &mut S,
        owner: Address,
        state: &crdt::Scalar<U256>,
    ) -> Result<(), CallHookError>
    where
        S: SlotStorage,
        S::Error: std::fmt::Display,
    {
        // Store projected - base modulo 2^256, retaining all four full-width fields.
        let delta = match state.delta {
            Some(delta) => <U256 as crdt::Type>::checked_apply(state.value, &delta)
                .ok_or_else(|| CallHookError::Handler("invalid scalar delta".into()))?
                .wrapping_sub(state.value),
            None => U256::ZERO,
        };

        for (slot, value) in [
            (self.min, state.limits.0),
            (self.max, state.limits.1),
            (self.value, state.value),
            (self.delta, delta),
        ] {
            storage
                .write(owner, slot, value)
                .map_err(|error| CallHookError::Handler(error.to_string()))?;
        }
        Ok(())
    }
}
