use alloy_primitives::{Address, U256};

use super::SlotStorage;
use crate::crdt;
use crate::execution::evm::{CallHookError, SlotCodec};

// Scalar field IDs in prefix byte 7.
#[derive(strum_macros::FromRepr)]
#[repr(u8)]
pub(in crate::execution::evm) enum Field {
    Min = 0,
    Max = 1,
    Value = 2,
    Delta = 3,
}

/// Maps a scalar state to its four EVM storage slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::execution::evm) struct ScalarStorage {
    pub min: U256,
    pub max: U256,
    pub value: U256,
    pub delta: U256,
}

impl ScalarStorage {
    /// Builds the scalar field keys from a validated scalar key.
    pub(in crate::execution::evm) fn new(key: U256) -> Self {
        Self {
            min: SlotCodec::with_field(key, Field::Min as u8),
            max: SlotCodec::with_field(key, Field::Max as u8),
            value: SlotCodec::with_field(key, Field::Value as u8),
            delta: SlotCodec::with_field(key, Field::Delta as u8),
        }
    }

    /// Loads the scalar and its storage mapping from a slot backend.
    pub(in crate::execution::evm) fn load<S>(
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

        // The base value disambiguates full-width additive and subtractive deltas.
        state.delta = (!delta.is_zero()).then(|| {
            <U256 as crdt::Type>::delta_between(state.value, state.value.wrapping_add(delta))
        });
        Ok((state, *self))
    }

    /// Stores the scalar at its mapped slots without applying its delta.
    pub(in crate::execution::evm) fn store<S>(
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
