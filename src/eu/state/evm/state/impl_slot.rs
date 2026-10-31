use alloy_evm::revm::{
    Database, DatabaseRef,
    state::{Account, EvmStorageSlot},
};
use alloy_primitives::{Address, U256};

use super::super::super::ApiStore;
use super::StateDb;

/// Reads and writes slots under their owning account in an EVM state map.
impl<DB: DatabaseRef> ApiStore for StateDb<DB> {
    type Error = DB::Error;

    fn read(&mut self, owner: Address, slot: U256) -> Result<U256, Self::Error> {
        self.storage(owner, slot)
    }

    fn write(&mut self, owner: Address, slot: U256, value: U256) -> Result<(), Self::Error> {
        let original = self.storage(owner, slot)?;
        let info = self.basic(owner)?.unwrap_or_default();
        let account = self
            .inner_mut()
            .entry(owner)
            .or_insert_with(|| Account::from(info));
        account.mark_touch();

        match account.storage.get_mut(&slot) {
            Some(current) => current.present_value = value,
            None => {
                account.storage.insert(
                    slot,
                    EvmStorageSlot::new_changed(original, value, Default::default()),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/slot.rs"]
mod tests;
