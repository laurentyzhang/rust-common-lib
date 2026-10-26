mod codec;
mod evm_internals;
pub(in crate::execution::evm) mod evm_state;
mod scalar;

use alloy_primitives::{Address, U256};

pub(in crate::execution::evm) use evm_internals::EvmInternalsStorage;
pub(in crate::execution::evm) use scalar::{Field, ScalarStorage};

/// Reads and writes account-scoped EVM storage slots.
pub(in crate::execution::evm) trait SlotStorage {
    type Error;

    fn read(&mut self, owner: Address, slot: U256) -> Result<U256, Self::Error>;

    fn write(&mut self, owner: Address, slot: U256, value: U256) -> Result<(), Self::Error>;
}
