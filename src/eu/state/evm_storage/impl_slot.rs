use std::convert::Infallible;

use alloy_evm::revm::state::{EvmStorage, EvmStorageSlot};
use alloy_primitives::{Address, U256};

use super::super::SlotStorage;

/// Reads and writes one account's storage map; `owner` is ignored.
impl SlotStorage for EvmStorage {
    type Error = Infallible;

    fn read(&mut self, _owner: Address, key: U256) -> Result<U256, Self::Error> {
        Ok(self
            .get(&key)
            .map(EvmStorageSlot::present_value)
            .unwrap_or_default())
    }

    fn write(&mut self, _owner: Address, key: U256, value: U256) -> Result<(), Self::Error> {
        match self.get_mut(&key) {
            // Preserve the slot's original value and transaction metadata.
            Some(slot) => slot.present_value = value,
            None => {
                self.insert(
                    key,
                    EvmStorageSlot::new_changed(U256::ZERO, value, Default::default()),
                );
            }
        }
        Ok(())
    }
}
