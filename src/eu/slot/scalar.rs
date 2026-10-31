//! Maps CRDT scalars to account-owned storage slots and encodes their pending deltas.

use alloy_primitives::{Address, U256};

use super::super::state::ApiStore;
use crate::crdt::state::DeltaOp;
use crate::crdt::{self, codecs::abi::AbiCodec};
use crate::eu::{CallHookError, KeyCodec};
use crate::namespace::Namespace;

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
    /// Checks if the given key corresponds to a delta field of a scalar.
    pub fn is_value_key(key: U256) -> bool {
        match KeyCodec::decode(key) {
            Some((namespace, field, _)) => {
                namespace == Namespace::Scalar
                    && matches!(Field::from_repr(field), Some(Field::Value))
            }
            None => false,
        }
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
        S: ApiStore,
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
        S: ApiStore,
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
