//! Maps CRDT scalars to account-owned storage slots and encodes their pending deltas.

use alloy_primitives::{Address, U256};

use super::super::state::SlotStorage;
use crate::crdt::state::DeltaOp;
use crate::crdt::{self, codecs::abi::AbiCodec};
use crate::eu::{ApiAddress, CallHookError, KeyCodec, key::Key};

/// Scalar field IDs encoded in byte 7 of a slot key.
#[derive(strum_macros::FromRepr)]
#[repr(u8)]
pub enum Field {
    Min = 0,
    Max = 1,
    Value = 2,
    PosDelta = 3,
    NegDelta = 4,
}

/// Holds the five slot keys for one scalar instance, rather than their stored values.
/// The owner address is supplied separately when accessing storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalarAdapter {
    pub min: U256,
    pub max: U256,
    pub value: U256,
    pub pos_delta: U256,
    pub neg_delta: U256,
}

// Read individual fields using any recognized field key of the scalar instance.
// Readers return None for invalid keys, backend failures, or values outside the requested type.
impl ScalarAdapter {
    /// Validates the scalar API and field ID, then derives all five field keys.
    pub(in crate::eu) fn parse(key: U256) -> Option<Self> {
        let (api, field, _) = KeyCodec::decode(key)?;
        if api != ApiAddress::Scalar || Field::from_repr(field).is_none() {
            return None;
        }
        Some(Self::new(key))
    }

    /// Reads the lower bound for the scalar identified by `key` under `owner`.
    pub fn read_min<T, S>(storage: &mut S, owner: Address, key: U256) -> Option<T>
    where
        T: AbiCodec,
        S: SlotStorage,
    {
        let slots = Self::parse(key)?;
        T::from_word(storage.read(owner, slots.min).ok()?)
    }

    /// Reads the upper bound for the scalar identified by `key` under `owner`.
    pub fn read_max<T, S>(storage: &mut S, owner: Address, key: U256) -> Option<T>
    where
        T: AbiCodec,
        S: SlotStorage,
    {
        let slots = Self::parse(key)?;
        T::from_word(storage.read(owner, slots.max).ok()?)
    }

    /// Reads the base value without applying the pending delta.
    pub fn read_value<T, S>(storage: &mut S, owner: Address, key: U256) -> Option<T>
    where
        T: AbiCodec,
        S: SlotStorage,
    {
        let slots = Self::parse(key)?;
        T::from_word(storage.read(owner, slots.value).ok()?)
    }

    /// Reads the positive and negative delta magnitudes as raw words, in that order.
    pub fn read_delta<S>(storage: &mut S, owner: Address, key: U256) -> Option<(U256, U256)>
    where
        S: SlotStorage,
    {
        let slots = Self::parse(key)?;
        Some((
            storage.read(owner, slots.pos_delta).ok()?,
            storage.read(owner, slots.neg_delta).ok()?,
        ))
    }
}

// Construct a slot layout and transfer whole scalar states through a backend.
impl ScalarAdapter {
    /// Derives all five field keys while preserving the instance key and prefix.
    /// The caller must validate the input key before calling this constructor.
    pub fn new(key: U256) -> Self {
        Self {
            min: KeyCodec::with_field(key, Field::Min as u8),
            max: KeyCodec::with_field(key, Field::Max as u8),
            value: KeyCodec::with_field(key, Field::Value as u8),
            pos_delta: KeyCodec::with_field(key, Field::PosDelta as u8),
            neg_delta: KeyCodec::with_field(key, Field::NegDelta as u8),
        }
    }

    /// Reads all five fields and reconstructs the scalar's net pending operation.
    /// Returns the scalar and a copy of this layout without applying the delta.
    pub fn load<T, S>(
        &self,
        storage: &mut S,
        owner: Address,
    ) -> Result<(crdt::Scalar<T>, Self), CallHookError>
    where
        T: AbiCodec,
        T::Magnitude: AbiCodec,
        S: SlotStorage,
        S::Error: std::fmt::Display,
    {
        // Read all five scalar fields from storage into a temporary array.
        let mut fields = [U256::ZERO; 5];
        for (slot, value) in [
            self.min,
            self.max,
            self.value,
            self.pos_delta,
            self.neg_delta,
        ]
        .into_iter()
        .zip(&mut fields)
        {
            *value = storage
                .read(owner, slot)
                .map_err(|error| CallHookError::Handler(error.to_string()))?;
        }

        let [min, max, value, pos_delta, neg_delta] = fields;

        let decode_value = |word| {
            T::from_word(word)
                .ok_or_else(|| CallHookError::Handler("scalar value out of range".into()))
        };

        let magnitude = |word| {
            T::Magnitude::from_word(word)
                .ok_or_else(|| CallHookError::Handler("scalar delta out of range".into()))
        };

        let delta = match pos_delta.cmp(&neg_delta) {
            std::cmp::Ordering::Greater => Some(DeltaOp::Add(magnitude(pos_delta - neg_delta)?)),
            std::cmp::Ordering::Less => Some(DeltaOp::Sub(magnitude(neg_delta - pos_delta)?)),
            std::cmp::Ordering::Equal => None,
        };

        let state = crdt::Scalar {
            min: decode_value(min)?,
            max: decode_value(max)?,
            value: decode_value(value)?,
            delta,
        };
        Ok((state, *self))
    }

    /// Writes bounds, base value, and separate positive and negative delta magnitudes.
    /// Checks pending arithmetic before writing and leaves the supplied scalar unchanged.
    pub fn store<T, S>(
        &self,
        storage: &mut S,
        owner: Address,
        state: &crdt::Scalar<T>,
    ) -> Result<(), CallHookError>
    where
        T: AbiCodec,
        T::Magnitude: AbiCodec,
        S: SlotStorage,
        S::Error: std::fmt::Display,
    {
        let (pos_delta, neg_delta) = match state.delta {
            Some(delta) => {
                T::checked_apply(state.value, &delta)
                    .ok_or_else(|| CallHookError::Handler("invalid scalar delta".into()))?;
                match delta {
                    DeltaOp::Add(magnitude) => (magnitude.to_word(), U256::ZERO),
                    DeltaOp::Sub(magnitude) => (U256::ZERO, magnitude.to_word()),
                }
            }
            None => (U256::ZERO, U256::ZERO),
        };

        for (slot, value) in [
            (self.min, state.min.to_word()),
            (self.max, state.max.to_word()),
            (self.value, state.value.to_word()),
            (self.pos_delta, pos_delta),
            (self.neg_delta, neg_delta),
        ] {
            storage
                .write(owner, slot, value)
                .map_err(|error| CallHookError::Handler(error.to_string()))?;
        }
        Ok(())
    }
}
