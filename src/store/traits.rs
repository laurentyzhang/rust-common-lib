use alloy_primitives::{Address, U256};

pub use super::error::StoreError;

pub trait Committer<K, V> {
    fn buffer(&mut self, id: K, state: V);
    /// Imports the accepted states and drops the rejected ones.
    fn stage(&mut self, accepted: Vec<K>) -> Result<(), StoreError>;
    /// Commit accepted states and clear the buffer on success.
    fn flush(&mut self) -> Result<(), StoreError>;
}

pub trait PhysicalStore<K, V>: Send + Sync {
    fn contains_key(&self, key: &K) -> Result<bool, StoreError> {
        self.get(key).map(|value| value.is_some())
    }

    fn get(&self, key: &K) -> Result<Option<V>, StoreError>;
}

/// Reads and writes 32-byte slots by owner and key, on whatever store sits underneath.
pub trait SlotStore {
    type Error;

    /// Reads `owner`'s slot `key`.
    fn read(&mut self, owner: Address, key: U256) -> Result<U256, Self::Error>;
    /// Writes `value` into `owner`'s slot `key`.
    fn write(&mut self, owner: Address, key: U256, value: U256) -> Result<(), Self::Error>;
}
