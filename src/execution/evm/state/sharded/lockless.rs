//! Lock-free batched updates of a `Sharded` state: route the updates to their shards, then write
//! each shard's batch.

use alloy_evm::revm::state::{Account, Bytecode, EvmState, EvmStorageSlot};
use alloy_primitives::map::DefaultHashBuilder;
use alloy_primitives::{Address, B256, U256};
use rayon::prelude::*;

use super::super::store::OverlayRead;
use super::{Sharded, account_shard, slot_shard};
use crate::execution::slot::set::{self, Property};

/// One shard's subset of the updates, from `partition`.
#[derive(Debug, Default)]
pub struct Batch {
    /// Account info, without storage.
    pub accounts: Vec<(Address, Account)>,
    pub slots: Vec<(Address, U256, EvmStorageSlot)>,
}

/// Lock-free batched updates: route the updates to their shards, then write each shard's batch.
pub trait LocklessWriter<const N: usize> {
    /// Splits `updates` into one subset per shard; only routes, takes no lock and writes nothing.
    fn partition(&self, updates: EvmState) -> [Batch; N];

    /// Writes each batch from `partition` into its shard, without locks.
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

    /// Writes each batch into its shard, all shards in parallel, through `&mut` without locks. An
    /// account's info replaces the stored info; its slots replace or add to the stored ones.
    fn write_batches(&mut self, batches: [Batch; N]) {
        self.shards
            .par_iter_mut()
            .zip(batches)
            .for_each(|(shard, batch)| {
                let shard = shard.get_mut().unwrap();
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

/// Read-only view of every shard, taken through `&mut Sharded` without locks; shared by the
/// threads that compute updates while nothing writes.
pub struct View<'a, const N: usize> {
    pub(super) shards: [&'a EvmState; N],
    pub(super) hasher: &'a DefaultHashBuilder,
}

impl<const N: usize> OverlayRead for View<'_, N> {
    /// Reads the account info from its address's shard.
    fn account<R>(&self, address: Address, f: impl FnOnce(Option<&Account>) -> R) -> R {
        f(self.shards[account_shard::<N>(self.hasher, address)].get(&address))
    }

    /// Reads the slot from its (address, key) shard.
    fn slot<R>(
        &self,
        address: Address,
        key: U256,
        f: impl FnOnce(Option<&EvmStorageSlot>) -> R,
    ) -> R {
        f(self.shards[slot_shard::<N>(self.hasher, address, key)]
            .get(&address)
            .and_then(|account| account.storage.get(&key)))
    }

    /// Searches every shard for the code.
    fn code(&self, code_hash: B256) -> Option<Bytecode> {
        self.shards.iter().find_map(|shard| {
            shard
                .values()
                .filter(|account| account.info.code_hash == code_hash)
                .find_map(|account| account.info.code.clone())
        })
    }
}

/// Writes a block's results into `cache` without locks: `changes` in the indexed format, then
/// what `persisted` writes to the store other than the indexed slots it clears, so the cache
/// keeps the list the store holds.
#[cfg_attr(not(feature = "mdbx"), allow(dead_code))]
pub(crate) fn update<const N: usize>(
    cache: &mut Sharded<N>,
    changes: EvmState,
    mut persisted: EvmState,
) {
    let batches = cache.partition(changes);
    cache.write_batches(batches);
    for account in persisted.values_mut() {
        account.storage.retain(|&key, _| {
            !matches!(
                set::KeyCodec::decode(key),
                Some((
                    _,
                    Property::Index | Property::Position | Property::Holes | Property::Indexed,
                    _
                ))
            )
        });
    }
    let batches = cache.partition(persisted);
    cache.write_batches(batches);
}
