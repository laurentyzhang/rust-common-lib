use alloy_evm::revm::{
    Database, DatabaseRef,
    state::{Account, EvmStorageSlot},
};
use alloy_primitives::{Address, U256};

use super::super::storage::ApiStore;
use super::StateDb;
use super::overlay::OverlayState;

/// Reads and writes slots under their owning account in an EVM state map.
impl<DB: DatabaseRef, Store: OverlayState> ApiStore for StateDb<DB, Store> {
    type Error = DB::Error;

    fn read(&mut self, owner: Address, slot: U256) -> Result<U256, Self::Error> {
        self.storage(owner, slot)
    }

    fn write(&mut self, owner: Address, slot: U256, value: U256) -> Result<(), Self::Error> {
        let original = self.storage(owner, slot)?;
        let info = self.basic(owner)?.unwrap_or_default();
        self.inner_mut()
            .account_mut(owner, || Account::from(info), Account::mark_touch);
        self.inner_mut().slot_mut(
            owner,
            slot,
            || EvmStorageSlot::new_changed(original, value, Default::default()),
            |current| current.present_value = value,
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/slot.rs"]
mod tests;
