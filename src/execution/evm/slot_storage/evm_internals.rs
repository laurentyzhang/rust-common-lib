use alloy_evm::EvmInternals;
use alloy_primitives::{Address, U256};

use super::SlotStorage;
use crate::execution::evm::CallHookError;

/// Adapts journal-backed [`EvmInternals`] to [`SlotStorage`].
pub(in crate::execution::evm) struct EvmInternalsStorage<'a, 'evm> {
    internals: &'a mut EvmInternals<'evm>,
}

impl<'a, 'evm> EvmInternalsStorage<'a, 'evm> {
    pub(in crate::execution::evm) fn new(internals: &'a mut EvmInternals<'evm>) -> Self {
        Self { internals }
    }
}

impl SlotStorage for EvmInternalsStorage<'_, '_> {
    type Error = CallHookError;

    fn read(&mut self, owner: Address, slot: U256) -> Result<U256, Self::Error> {
        self.internals
            .sload(owner, slot)
            .map(|value| value.data)
            .map_err(|error| CallHookError::Handler(error.to_string()))
    }

    fn write(&mut self, owner: Address, slot: U256, value: U256) -> Result<(), Self::Error> {
        self.internals
            .touch_account(owner)
            .map_err(|error| CallHookError::Handler(error.to_string()))?;
        self.internals
            .sstore(owner, slot, value)
            .map(|_| ())
            .map_err(|error| CallHookError::Handler(error.to_string()))
    }
}
