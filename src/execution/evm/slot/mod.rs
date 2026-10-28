mod evm_internals;
mod evm_state;
mod evm_storage;
mod key_codec;
mod scalar;

#[cfg(test)]
mod tests;

use alloy_primitives::{Address, U256};

pub use key_codec::KeyCodec;
pub(in crate::execution::evm) use key_codec::{API_OFFSET, FIELD_OFFSET};
pub(in crate::execution::evm) use scalar::{Field, ScalarSlots};

/// Reads and writes account-scoped EVM storage slots.
pub(in crate::execution::evm) trait SlotStorage {
    type Error;

    fn read(&mut self, owner: Address, key: U256) -> Result<U256, Self::Error>;

    fn write(&mut self, owner: Address, key: U256, value: U256) -> Result<(), Self::Error>;
}
