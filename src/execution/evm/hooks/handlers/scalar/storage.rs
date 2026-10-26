use crate::crdt;
use crate::execution::evm::{CallHookError, SlotCodec};
use alloy_evm::EvmInternals;
use alloy_primitives::{Address, U256};

// Scalar field IDs in prefix byte 7.
#[derive(strum_macros::FromRepr)]
#[repr(u8)]
pub(super) enum Field {
    Min = 0,
    Max = 1,
    Value = 2,
    Delta = 3,
}

/// Storage keys for scalar fields, not the values stored at those keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) struct SlotKeys {
    pub min: U256,
    pub max: U256,
    pub value: U256,
    pub delta: U256,
}

impl SlotKeys {
    /// Builds the field keys from a validated scalar key.
    pub(in super::super) fn new(key: U256) -> Self {
        Self {
            min: SlotCodec::with_field(key, Field::Min as u8),
            max: SlotCodec::with_field(key, Field::Max as u8),
            value: SlotCodec::with_field(key, Field::Value as u8),
            delta: SlotCodec::with_field(key, Field::Delta as u8),
        }
    }

    /// Loads the scalar and its original slot keys from the current journal state.
    pub(in super::super) fn load(
        &self,
        internals: &mut EvmInternals<'_>,
        owner: Address,
    ) -> Result<(crdt::Scalar<U256>, Self), CallHookError> {
        let mut state = crdt::Scalar::<U256>::default();
        let mut delta = U256::ZERO;
        for (slot, value) in [
            (self.min, &mut state.limits.0),
            (self.max, &mut state.limits.1),
            (self.value, &mut state.value),
            (self.delta, &mut delta),
        ] {
            *value = internals
                .sload(owner, slot)
                .map_err(|error| CallHookError::Handler(error.to_string()))?
                .data;
        }

        // The base value disambiguates full-width additive and subtractive deltas.
        state.delta = (!delta.is_zero()).then(|| {
            <U256 as crdt::Type>::delta_between(state.value, state.value.wrapping_add(delta))
        });
        Ok((state, *self))
    }

    /// Stores the scalar at these keys without applying its delta.
    /// Returns the named keys that were written.
    pub(in super::super) fn store(
        &self,
        internals: &mut EvmInternals<'_>,
        owner: Address,
        state: &crdt::Scalar<U256>,
    ) -> Result<(), CallHookError> {
        // Store projected - base modulo 2^256, retaining all four full-width fields.
        let delta = match state.delta {
            Some(delta) => <U256 as crdt::Type>::checked_apply(state.value, &delta)
                .ok_or_else(|| CallHookError::Handler("invalid scalar delta".into()))?
                .wrapping_sub(state.value),
            None => U256::ZERO,
        };

        internals
            .touch_account(owner)
            .map_err(|error| CallHookError::Handler(error.to_string()))?;

        for (slot, value) in [
            (self.min, state.limits.0),
            (self.max, state.limits.1),
            (self.value, state.value),
            (self.delta, delta),
        ] {
            internals
                .sstore(owner, slot, value)
                .map_err(|error| CallHookError::Handler(error.to_string()))?;
        }
        Ok(())
    }
}
