use alloy_evm::revm::state::{Account, EvmState};
use alloy_primitives::{Address, U256};

pub trait AccountStorage {
    /// Return the account at the requested address.
    fn get(&self, address: &Address) -> Option<&Account>;

    /// Iterate over the accounts.
    fn accounts(&self) -> impl Iterator<Item = &Account>;

    /// Return the owned state for persistence.
    fn into_evm_state(self) -> EvmState;
}

/// Reads and writes account-scoped EVM storage slots.
pub(in crate::execution::evm) trait SlotStorage {
    type Error;

    fn read(&mut self, owner: Address, key: U256) -> Result<U256, Self::Error>;
    fn write(&mut self, owner: Address, key: U256, value: U256) -> Result<(), Self::Error>;
}
