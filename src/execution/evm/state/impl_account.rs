use alloy_evm::revm::{
    DatabaseRef,
    state::{Account, AccountInfo, EvmState},
};
use alloy_primitives::Address;

use super::super::storage::AccountStorage;
use super::StateDb;

impl<DB: DatabaseRef> AccountStorage for StateDb<DB> {
    type Error = DB::Error;

    fn get(&self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.basic_ref(*address)
    }

    /// Iterate over locally stored accounts.
    fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.inner().values()
    }

    /// Return the owned local state for persistence.
    fn into_evm_state(self) -> EvmState {
        self.into_inner()
    }
}
