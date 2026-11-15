use alloy_evm::revm::{DatabaseCommit, DatabaseRef, state::EvmState};

use super::StateDb;

impl<DB: DatabaseRef> DatabaseCommit for StateDb<DB> {
    fn commit(&mut self, other: EvmState) {
        if self.commit_error.is_none() {
            self.commit_error = self.overlay.apply(other, self.fallback.as_ref()).err();
        }
    }
}

#[cfg(test)]
#[path = "tests/commit.rs"]
mod tests;
