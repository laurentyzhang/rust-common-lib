use alloy_evm::revm::state::{Account, AccountInfo, EvmState};
use alloy_primitives::{Address, U256};

pub trait AccountStorage {
    type Error;

    /// Read account information from local state or the fallback.
    fn get(&self, address: &Address) -> Result<Option<AccountInfo>, Self::Error>;

    /// Iterate over locally stored accounts.
    fn accounts(&self) -> impl Iterator<Item = &Account>;

    /// Return the owned local state for persistence.
    fn into_evm_state(self) -> EvmState;
}

/// Trait for reading and writing key-value pairs for the APIs.
pub(in crate::eu) trait ApiStore {
    type Error;

    fn read(&mut self, owner: Address, key: U256) -> Result<U256, Self::Error>;
    fn write(&mut self, owner: Address, key: U256, value: U256) -> Result<(), Self::Error>;
}
