use alloy_evm::revm::{DatabaseCommit, DatabaseRef, state::EvmState};

use crate::store::StoreError;

use super::StateDb;
use super::overlay::OverlayState;

impl<DB: DatabaseRef, Store: OverlayState> DatabaseCommit for StateDb<DB, Store> {
    fn commit(&mut self, other: EvmState) {
        if self.commit_error.is_none() {
            self.commit_error = self.overlay.apply(other, self.fallback.as_ref()).err();
        }
    }
}

impl<DB: DatabaseRef, Store: OverlayState> StateDb<DB, Store> {
    pub(in crate::execution) fn commit_with_original_slots(
        &mut self,
        other: EvmState,
    ) -> Result<(), StoreError> {
        self.overlay
            .apply_with_fallbacks(other, self.fallback.as_ref(), None)
    }
}

#[cfg(test)]
#[path = "tests/commit.rs"]
mod tests;
