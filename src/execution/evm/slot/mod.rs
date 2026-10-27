mod codec;
mod evm_internals;
mod evm_state;
mod evm_storage;
mod scalar;

use alloy_primitives::{Address, U256};

pub(in crate::execution::evm) use scalar::{Field, ScalarSlot};

/// Reads and writes account-scoped EVM storage slots.
pub(in crate::execution::evm) trait SlotStorage {
    type Error;

    fn read(&mut self, owner: Address, key: U256) -> Result<U256, Self::Error>;

    fn write(&mut self, owner: Address, key: U256, value: U256) -> Result<(), Self::Error>;
}
