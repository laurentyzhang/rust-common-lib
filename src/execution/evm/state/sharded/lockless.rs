//! Lock-free batched updates of a `Sharded` state: route the updates to their shards, then write
//! each shard's batch.

use alloy_evm::revm::DatabaseRef;
use alloy_evm::revm::state::{Account, Bytecode, EvmState, EvmStorageSlot};
use alloy_primitives::map::DefaultHashBuilder;
use alloy_primitives::{Address, B256, U256};
use rayon::prelude::*;

use super::super::StateDb;
use super::super::overlay::{StateOverlay, without_storage};
use super::super::store::OverlayRead;
use super::{Sharded, account_shard, slot_shard};
use crate::execution::slot::CrdtSlot;
use crate::execution::slot::set::{self, Property};
use crate::store::StoreError;

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
            batches[self.index(address)].accounts.push((address, account));
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
struct View<'a, const N: usize> {
    shards: [&'a EvmState; N],
    hasher: &'a DefaultHashBuilder,
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

    /// Does nothing: the view is read-only.
    fn cache_account(&self, _address: Address, _account: Account) {}

    /// Does nothing: the view is read-only.
    fn cache_slot(&self, _address: Address, _key: U256, _slot: EvmStorageSlot) {}

    /// Does nothing: the view is read-only.
    fn convert_on_read(&self, _address: Address, _key: U256, _load: impl Fn(U256) -> Option<U256>) {}
}

/// Applies `crdt`'s deltas into `db`'s shards without locks: every owner's writes are computed in
/// parallel against a read-only view, then partitioned and written shard by shard.
pub(super) fn apply_crdt<DB: DatabaseRef + Sync, const N: usize>(
    db: &mut StateDb<DB, Sharded<N>>,
    crdt: &mut CrdtSlot,
) -> Result<(), StoreError> {
    let fallback = db.fallback.as_ref();
    let sharded = db.overlay.inner_mut();
    let updates = {
        let Sharded { shards, hasher, .. } = &mut *sharded;
        let view = View {
            shards: shards.each_mut().map(|shard| &*shard.get_mut().unwrap()),
            hasher,
        };
        let base = StateDb::with_fallback(
            StateOverlay::new(view, StateOverlay::copy_balance_nonce),
            fallback,
        );
        // Phase 1: each owner writes into its own overlay over the view.
        let stores = crdt.apply_by_owner(|_| {
            StateDb::with_fallback(
                StateOverlay::new(EvmState::default(), StateOverlay::copy_balance_nonce),
                Some(&base),
            )
        })?;
        let mut updates = EvmState::default();
        for store in stores {
            for (address, mut account) in store.into_inner() {
                // Keep the stored account and the stored slots' original values; only mark the
                // account touched.
                if let Some(mut current) = base
                    .inner()
                    .account(address, |current| current.map(without_storage))
                {
                    current.storage = std::mem::take(&mut account.storage);
                    current.mark_touch();
                    account = current;
                }
                for (&key, slot) in account.storage.iter_mut() {
                    if let Some(original) =
                        base.inner().slot(address, key, |kept| kept.map(|kept| kept.original_value))
                    {
                        slot.original_value = original;
                    }
                }
                updates.insert(address, account);
            }
        }
        updates
    };
    // Phase 2: write the updates shard by shard.
    let batches = sharded.partition(updates);
    sharded.write_batches(batches);
    Ok(())
}

/// Writes a block's results into `cache` without locks: `changes` in the execution format, then
/// what `persisted` writes to the store other than the execution format it clears, so the cache
/// keeps the list the store holds.
pub(crate) fn update<const N: usize>(cache: &mut Sharded<N>, changes: EvmState, mut persisted: EvmState) {
    let batches = cache.partition(changes);
    cache.write_batches(batches);
    for account in persisted.values_mut() {
        account.storage.retain(|&key, _| {
            !matches!(
                set::KeyCodec::decode(key),
                Some((_, Property::Index | Property::Position | Property::Holes | Property::Indexed, _))
            )
        });
    }
    let batches = cache.partition(persisted);
    cache.write_batches(batches);
}
