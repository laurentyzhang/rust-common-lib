use std::convert::Infallible;

use alloy_evm::revm::state::{Account, EvmState, EvmStorageSlot};
use alloy_primitives::{Address, U256};

use super::SlotStorage;

/// Adapts a mutable [`EvmState`] map to [`SlotStorage`].
pub(in crate::execution::evm) struct EvmStateStorage<'a> {
    state: &'a mut EvmState,
}

impl<'a> EvmStateStorage<'a> {
    pub(in crate::execution::evm) fn new(state: &'a mut EvmState) -> Self {
        Self { state }
    }
}

impl SlotStorage for EvmStateStorage<'_> {
    type Error = Infallible;

    fn read(&mut self, owner: Address, slot: U256) -> Result<U256, Self::Error> {
        Ok(self
            .state
            .get(&owner)
            .filter(|account| {
                !account.is_selfdestructed()
                    && (!account.is_loaded_as_not_existing() || account.is_created())
            })
            .and_then(|account| account.storage.get(&slot))
            .map(EvmStorageSlot::present_value)
            .unwrap_or_default())
    }

    fn write(&mut self, owner: Address, slot: U256, value: U256) -> Result<(), Self::Error> {
        let account = self.state.entry(owner).or_insert_with(Account::default);
        account.mark_touch();
        match account.storage.get_mut(&slot) {
            Some(current) => current.present_value = value,
            None => {
                account.storage.insert(
                    slot,
                    EvmStorageSlot::new_changed(U256::ZERO, value, Default::default()),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_present_values() {
        let owner = Address::repeat_byte(0x11);
        let slot = U256::from(7);
        let mut state = EvmState::default();
        let mut storage = EvmStateStorage::new(&mut state);

        assert_eq!(storage.read(owner, slot).unwrap(), U256::ZERO);
        storage.write(owner, slot, U256::from(10)).unwrap();
        storage.write(owner, slot, U256::from(20)).unwrap();
        assert_eq!(storage.read(owner, slot).unwrap(), U256::from(20));

        let account = state.get(&owner).unwrap();
        assert!(account.is_touched());
        let stored = account.storage.get(&slot).unwrap();
        assert_eq!(stored.original_value(), U256::ZERO);
        assert_eq!(stored.present_value(), U256::from(20));
    }
}
