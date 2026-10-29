use alloy_evm::revm::state::{Account, EvmState};
use alloy_primitives::Address;

use super::super::super::AccountStorage;
use super::StateDb;

impl AccountStorage for StateDb {
    /// Return the account at the requested address.
    fn get(&self, address: &Address) -> Option<&Account> {
        self.inner().get(address)
    }

    /// Iterate over the accounts.
    fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.inner().values()
    }

    /// Return the owned state for persistence.
    fn into_evm_state(self) -> EvmState {
        self.into_inner()
    }
}
