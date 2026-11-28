//! Batched updates of a `Sharded` state: route the updates to their shards, then write each
//! shard's batch, all shards in parallel.

use alloy_evm::revm::state::{Account, EvmState, EvmStorageSlot};
use alloy_primitives::{Address, U256};
use rayon::prelude::*;

use super::Sharded;

/// One shard's subset of the updates, from `partition`.
#[derive(Debug, Default)]
pub struct Batch {
    /// Account info, without storage.
    pub accounts: Vec<(Address, Account)>,
    pub slots: Vec<(Address, U256, EvmStorageSlot)>,
}

/// Batched updates: route the updates to their shards, then write each shard's batch.
pub trait LocklessWriter<const N: usize> {
    /// Splits `updates` into one subset per shard; only routes and writes nothing.
    fn partition(&self, updates: EvmState) -> [Batch; N];

    /// Writes each batch from `partition` into its shard.
    fn write_batches(&mut self, batches: [Batch; N]);
}

impl<const N: usize> LocklessWriter<N> for Sharded<N> {
    /// Splits `updates` into one subset per shard: an account's info goes to its address's shard,
    /// each slot to its (address, key) shard.
    fn partition(&self, updates: EvmState) -> [Batch; N] {
        let mut batches: [Batch; N] = std::array::from_fn(|_| Batch::default());
        for (address, mut account) in updates {
            for (key, slot) in std::mem::take(&mut account.storage) {
                batches[self.slot_index(address, key)]
                    .slots
                    .push((address, key, slot));
            }
            batches[self.index(address)]
                .accounts
                .push((address, account));
        }
        batches
    }

    /// Writes each batch into its shard, all shards in parallel. An account's info replaces the
    /// stored info; its slots replace or add to the stored ones.
    fn write_batches(&mut self, batches: [Batch; N]) {
        self.shards
            .par_iter_mut()
            .zip(batches)
            .for_each(|(shard, batch)| {
                for (address, mut account) in batch.accounts {
                    let current = shard.entry(address).or_default();
                    account.storage = std::mem::take(&mut current.storage);
                    *current = account;
                }
                for (address, key, slot) in batch.slots {
                    shard.entry(address).or_default().storage.insert(key, slot);
                }
            });
    }
}
