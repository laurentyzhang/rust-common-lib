use alloy_primitives::{Address, U256};

/// Trait for reading and writing key-value pairs for the APIs.
pub trait ApiStore {
    type Error;

    fn read(&mut self, owner: Address, key: U256) -> Result<U256, Self::Error>;
    fn write(&mut self, owner: Address, key: U256, value: U256) -> Result<(), Self::Error>;
}
