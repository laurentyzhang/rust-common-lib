use alloy_evm::revm::{DatabaseCommit, DatabaseRef, state::EvmState};

use super::StateDb;

impl<DB: DatabaseRef> DatabaseCommit for StateDb<DB> {
    fn commit(&mut self, other: EvmState) {
        self.overlay
            .apply(other, self.fallback.as_ref())
            .expect("failed to apply EVM state");
    }
}

#[cfg(test)]
#[path = "tests/commit.rs"]
mod tests;
