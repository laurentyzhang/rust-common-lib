use alloy_evm::revm::{
    DatabaseCommit,
    state::{AccountStatus, EvmState},
};

use super::StateDb;

impl<DB> DatabaseCommit for StateDb<DB> {
    fn commit(&mut self, changes: EvmState) {
        for (address, mut incoming) in changes {
            if incoming.is_selfdestructed() {
                self.inner_mut().insert(address, incoming);
                continue;
            }

            if incoming.is_loaded_as_not_existing() && incoming.is_touched() {
                incoming.status.remove(AccountStatus::LoadedAsNotExisting);
            }

            if let Some(existing) = self.inner_mut().get_mut(&address) {
                if !incoming.is_created() && !existing.is_selfdestructed() {
                    // Preserve slots absent from this transaction's update.
                    let mut storage = std::mem::take(&mut existing.storage);
                    storage.extend(incoming.storage);
                    incoming.storage = storage;
                    if existing.is_created() {
                        incoming.mark_created();
                    }
                    if incoming.info.code.is_none()
                        && incoming.info.code_hash == existing.info.code_hash
                    {
                        incoming.info.code = existing.info.code.clone();
                    }
                }
            }

            self.inner_mut().insert(address, incoming);
        }
    }
}

#[cfg(test)]
#[path = "tests/commit.rs"]
mod tests;
