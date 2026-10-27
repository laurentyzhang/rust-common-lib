use alloy_evm::EvmInternals;
use alloy_primitives::{Address, U256};

use super::SlotStorage;
use crate::execution::evm::CallHookError;

/// Reads and writes slots through the current EVM journal.
impl SlotStorage for EvmInternals<'_> {
    type Error = CallHookError;

    fn read(&mut self, owner: Address, slot: U256) -> Result<U256, Self::Error> {
        self.sload(owner, slot)
            .map(|value| value.data)
            .map_err(|error| CallHookError::Handler(error.to_string()))
    }

    fn write(&mut self, owner: Address, slot: U256, value: U256) -> Result<(), Self::Error> {
        self.touch_account(owner)
            .map_err(|error| CallHookError::Handler(error.to_string()))?;
        self.sstore(owner, slot, value)
            .map(|_| ())
            .map_err(|error| CallHookError::Handler(error.to_string()))
    }
}
