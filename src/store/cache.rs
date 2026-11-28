//! `PhysicalCachedStore`, sharded quick_caches over a physical_store, and `PersistentStore`, its
//! EVM form over MDBX, which keeps accounts, slots and whole Sets in the execution format.
//!
//! Only MDBX keeps Sets compact; the cache and every layer above hold them expanded. A Set miss
//! expands through `Expand`. The cache holds only committed values with no pending deltas, the
//! same data MDBX holds: `PhysicalStoreCacheCommitter` writes it in parallel with MDBX's
//! committer, from the same accepted batches.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Debug;
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::sync::RwLock;

use alloy_evm::revm::{
    DatabaseRef,
    primitives::KECCAK_EMPTY,
    state::{Account, AccountInfo, Bytecode, EvmState, EvmStorageSlot, TransactionId},
};
use alloy_primitives::map::{DefaultHashBuilder, HashMap};
use alloy_primitives::{Address, B256, U256};
use quick_cache::{Lifecycle, UnitWeighter, Weighter, unsync};
use rayon::prelude::*;

use crate::store::SlotStore;
use crate::execution::evm::state::store::merge;
use crate::execution::evm::state::{StateDb, StateOverlay, store::OverlayRead};
use crate::execution::workload::ID;
use crate::execution::{CrdtSlot, FormatConverter, set};
use crate::store::traits::{Committer, PhysicalStore, StoreError};

/// Converts a missing entry from a physical_store's format into a cache's, as the cache loads it.
pub trait Load<K, V> {
    /// Loads `key` from `physical_store` in the cache's format; `None` when the physical_store lacks it.
    fn load(physical_store: &dyn PhysicalStore<K, V>, key: &K) -> Result<Option<V>, StoreError>;
}

/// Loads entries as the physical_store holds them, for a physical_store in the cache's own format.
#[derive(Debug, Default)]
pub struct Plain;

/// Reads the key from the physical_store unchanged.
impl<K, V> Load<K, V> for Plain {
    /// Returns the physical_store's value as is.
    fn load(physical_store: &dyn PhysicalStore<K, V>, key: &K) -> Result<Option<V>, StoreError> {
        physical_store.get(key)
    }
}

/// A cache split into `N` shards, each a quick_cache behind its own lock, over an optional
/// physical_store whose entries `Lo` converts as they load. Reads and misses lock one shard; `flush`
/// writes through `&mut` without locks.
pub struct PhysicalCachedStore<
    'a,
    K,
    V,
    We = UnitWeighter,
    L = unsync::DefaultLifecycle<K, V>,
    Lo = Plain,
    const N: usize = 64,
> {
    /// The quick_caches; a key lives in the shard `index` picks.
    shards: [RwLock<unsync::Cache<K, V, We, quick_cache::DefaultHashBuilder, L>>; N],
    /// Picks a key's shard.
    hasher: DefaultHashBuilder,
    /// The shards' eviction policy, kept for policy state shared across shards.
    lifecycle: L,
    /// Where misses load from.
    physical_store: Option<&'a dyn PhysicalStore<K, V>>,
    /// The physical_store-to-cache conversion, fixed by the type.
    load: PhantomData<Lo>,
}

/// Construction, reads with loading on a miss, and lock-free writes.
impl<'a, K: Eq + Hash, V, We: Weighter<K, V>, L: Lifecycle<K, V>, Lo: Load<K, V>, const N: usize>
    PhysicalCachedStore<'a, K, V, We, L, Lo, N>
{
    /// Creates a cache holding up to `capacity` in weight, split evenly over the shards.
    pub fn new(
        capacity: u64,
        weighter: We,
        lifecycle: L,
        physical_store: Option<&'a dyn PhysicalStore<K, V>>,
    ) -> Self
    where
        We: Clone,
        L: Clone,
    {
        let per_shard = capacity.div_ceil(N as u64).max(1);
        Self {
            shards: std::array::from_fn(|_| {
                RwLock::new(unsync::Cache::with(
                    per_shard as usize,
                    per_shard,
                    weighter.clone(),
                    Default::default(),
                    lifecycle.clone(),
                ))
            }),
            hasher: DefaultHashBuilder::default(),
            lifecycle,
            physical_store,
            load: PhantomData,
        }
    }

    /// Detaches the physical_store, keeping the cached entries.
    pub fn into_cache<'b>(self) -> PhysicalCachedStore<'b, K, V, We, L, Lo, N> {
        PhysicalCachedStore {
            shards: self.shards,
            hasher: self.hasher,
            lifecycle: self.lifecycle,
            physical_store: None,
            load: PhantomData,
        }
    }

    /// Attaches `physical_store` to the entries of `cache`.
    pub fn from_cache(
        cache: PhysicalCachedStore<'_, K, V, We, L, Lo, N>,
        physical_store: Option<&'a dyn PhysicalStore<K, V>>,
    ) -> Self {
        Self {
            shards: cache.shards,
            hasher: cache.hasher,
            lifecycle: cache.lifecycle,
            physical_store,
            load: PhantomData,
        }
    }

    /// Returns the index of the shard holding `key`.
    fn index(&self, key: &K) -> usize {
        self.hasher.hash_one(key) as usize % N
    }

    /// Reads `key` with `f` under its shard's read lock; on a miss, loads it from the physical_store
    /// through `Lo` and keeps it, all under the shard's write lock, so only one reader loads a key.
    /// A key the physical_store lacks reads as `None` and is not kept.
    pub fn get_with<R>(&self, key: &K, f: impl FnOnce(&V) -> R) -> Result<Option<R>, StoreError>
    where
        K: Clone,
    {
        let shard = &self.shards[self.index(key)];
        if let Some(value) = shard.read().unwrap().get(key) {
            return Ok(Some(f(value)));
        }
        let mut shard = shard.write().unwrap();
        // Another reader may have loaded it while this one waited for the lock.
        if let Some(value) = shard.get(key) {
            return Ok(Some(f(value)));
        }
        let Some(physical_store) = self.physical_store else {
            return Ok(None);
        };
        let Some(value) = Lo::load(physical_store, key)? else {
            return Ok(None);
        };
        let result = f(&value);
        shard.insert(key.clone(), value);
        Ok(Some(result))
    }

    /// Writes `updates` through `&mut`, without locks, every shard in parallel: `merge` gets the
    /// cached value, if any, and returns what to insert for a missing key. Returns each shard's
    /// eviction record.
    pub fn flush(
        &mut self,
        updates: Vec<(K, V)>,
        merge: impl Fn(Option<&mut V>, V) -> Option<V> + Sync,
    ) -> Vec<L::RequestState>
    where
        K: Send,
        V: Send,
        We: Send,
        L: Send,
        L::RequestState: Send,
    {
        let mut batches: [Vec<(K, V)>; N] = std::array::from_fn(|_| Vec::new());
        for (key, value) in updates {
            batches[self.index(&key)].push((key, value));
        }
        self.shards
            .par_iter_mut()
            .zip(batches)
            .map(|(shard, batch)| {
                let shard = shard.get_mut().unwrap();
                let mut evicted = L::RequestState::default();
                for (key, value) in batch {
                    let inserted = match shard.get_mut(&key) {
                        Some(mut current) => merge(Some(&mut current), value),
                        None => merge(None, value),
                    };
                    if let Some(value) = inserted {
                        shard.insert_with_lifecycle(key, value, &mut evicted);
                    }
                }
                evicted
            })
            .collect()
    }

    /// Drops `keys`, without locks.
    pub fn remove(&mut self, keys: &[K]) {
        for key in keys {
            let index = self.index(key);
            self.shards[index].get_mut().unwrap().remove(key);
        }
    }
}

/// A `Debug` that needs no bounds on the parameters.
impl<K, V, We, L, Lo, const N: usize> Debug for PhysicalCachedStore<'_, K, V, We, L, Lo, N> {
    /// Shows the shard count; the shards and the physical_store have no useful `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhysicalCachedStore")
            .field("shards", &N)
            .finish_non_exhaustive()
    }
}

/// A key of the EVM cache: an account's info, a plain slot, a whole Set, code, or a block hash.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CacheKey {
    /// An account's info, by address.
    Account(Address),
    /// A plain slot, by owner and slot key.
    Slot(Address, U256),
    /// A whole Set, by owner and Set id.
    Set(Address, [u8; 6]),
    /// Code, by its hash.
    Code(B256),
    /// A block hash, by block number.
    BlockHash(u64),
}

/// A value of the EVM cache.
#[derive(Clone, Debug)]
pub enum CacheVal {
    /// The account without its storage.
    Account(Account),
    /// A plain slot.
    Slot(EvmStorageSlot),
    /// Every slot of one Set, in the execution format.
    Set(HashMap<U256, EvmStorageSlot>),
    /// The code.
    Code(Bytecode),
    /// The block hash.
    BlockHash(B256),
}

/// Weighs the EVM cache's entries.
#[derive(Clone, Copy, Debug, Default)]
pub struct ByWeight;

/// Sizes Sets by their slots, so a large Set costs its share of the capacity.
impl Weighter<CacheKey, CacheVal> for ByWeight {
    /// One per slot of a Set, one for any other entry.
    fn weight(&self, _key: &CacheKey, value: &CacheVal) -> u64 {
        match value {
            CacheVal::Set(slots) => slots.len().max(1) as u64,
            _ => 1,
        }
    }
}

/// The persistent store execution reads: the cache over MDBX, one store to callers.
pub type PersistentStore<'a, const N: usize = 64> = PhysicalCachedStore<
    'a,
    CacheKey,
    CacheVal,
    ByWeight,
    unsync::DefaultLifecycle<CacheKey, CacheVal>,
    Expand,
    N,
>;

/// Splits execution-format changes into cache entries: accounts, plain slots, code, and the
/// changed slots of each Set.
pub(crate) fn entries(changes: EvmState) -> Vec<(CacheKey, CacheVal)> {
    let mut entries = Vec::new();
    let mut sets: HashMap<(Address, [u8; 6]), HashMap<U256, EvmStorageSlot>> = HashMap::default();
    for (address, mut account) in changes {
        for (key, slot) in std::mem::take(&mut account.storage) {
            match set::KeyCodec::decode(key) {
                Some((set, ..)) => {
                    sets.entry((address, set)).or_default().insert(key, slot);
                }
                None => entries.push((CacheKey::Slot(address, key), CacheVal::Slot(slot))),
            }
        }
        let info = &account.info;
        if let Some(code) = info.code.clone().filter(|_| info.code_hash != KECCAK_EMPTY) {
            entries.push((CacheKey::Code(info.code_hash), CacheVal::Code(code)));
        }
        entries.push((CacheKey::Account(address), CacheVal::Account(account)));
    }
    for ((address, set), slots) in sets {
        entries.push((CacheKey::Set(address, set), CacheVal::Set(slots)));
    }
    entries
}

/// Writes committed changes into the cache.
impl<const N: usize> PersistentStore<'_, N> {
    /// Writes `entries` into what the cache holds, without locks: an entry it doesn't hold is
    /// skipped, as it loads from MDBX on its next read. A Set takes the changed slots, an empty
    /// one leaving it; any other entry is replaced.
    pub fn write(&mut self, entries: Vec<(CacheKey, CacheVal)>) -> Result<(), StoreError> {
        self.flush(entries, |current, new| {
            match (current, new) {
                (Some(CacheVal::Set(slots)), CacheVal::Set(new)) => {
                    for (key, slot) in new {
                        match slot.present_value.is_zero() {
                            true => slots.remove(&key),
                            false => slots.insert(key, slot),
                        };
                    }
                }
                (Some(current), new) => *current = new,
                (None, _) => {}
            }
            None
        });
        Ok(())
    }
}

/// Commits accepted batches into the cache in front of MDBX, in parallel with MDBX's committer,
/// with the same buffer, stage and flush: native values as they are, CRDT values with their
/// deltas applied and Sets reindexed. A value the cache doesn't hold is skipped; it loads from MDBX
/// on its next read.
#[derive(Debug)]
pub struct PhysicalStoreCacheCommitter<'a> {
    buffered: BTreeMap<u64, EvmState>,
    staged: Vec<(u64, EvmState)>,
    cache: PersistentStore<'a>,
}

impl<'a> PhysicalStoreCacheCommitter<'a> {
    /// Commits into `cache`.
    pub fn new(cache: PersistentStore<'a>) -> Self {
        Self {
            buffered: BTreeMap::new(),
            staged: Vec::new(),
            cache,
        }
    }

    /// Returns the cache, for reads between commits.
    pub fn cache(&self) -> &PersistentStore<'a> {
        &self.cache
    }
}

impl Committer<u64, EvmState> for PhysicalStoreCacheCommitter<'_> {
    /// Holds a batch's changes until it is accepted or rejected.
    fn buffer(&mut self, batch: u64, changes: EvmState) {
        self.buffered.insert(batch, changes);
    }

    /// Keeps the accepted batches, in batch order, and drops the rest.
    fn stage(&mut self, accepted: Vec<u64>) -> Result<(), StoreError> {
        let accepted = accepted.into_iter().collect::<HashSet<_>>();
        for (batch, changes) in std::mem::take(&mut self.buffered) {
            if accepted.contains(&batch) {
                self.staged.push((batch, changes));
            }
        }
        Ok(())
    }

    /// Merges the staged batches, applies their CRDT deltas to the values the cache holds, and
    /// writes the cache. An account the batches created or destroyed is dropped, so it loads again
    /// from MDBX without the batches' status.
    fn flush(&mut self) -> Result<(), StoreError> {
        let mut crdt = CrdtSlot::default();
        let mut merged = EvmState::default();
        for (batch, changes) in std::mem::take(&mut self.staged) {
            let id = ID {
                block: batch,
                ..Default::default()
            };
            merge(&mut merged, crdt.merge_delta_from(id, changes)?);
        }
        // Detached from MDBX, which takes this commit in parallel, reads see only what the cache holds.
        let physical_store = self.cache.physical_store.take();
        let mut db = StateDb::with_fallback(
            StateOverlay::new(merged, StateOverlay::copy_balance_nonce),
            Some(&self.cache),
        );
        let reindexed = crdt
            .drain()
            .into_iter()
            .map(|(owner, sets, scalars)| Ok((owner, CrdtSlot::reindex(&mut db, owner, sets, scalars)?)))
            .collect::<Result<Vec<_>, StoreError>>();
        let mut merged = db.into_inner();
        self.cache.physical_store = physical_store;
        for (owner, slots) in reindexed? {
            if let Some(account) = merged.get_mut(&owner) {
                account.storage.extend(slots);
            }
        }
        let wiped: Vec<CacheKey> = merged
            .iter()
            .filter(|(_, account)| account.is_created() || account.is_selfdestructed())
            .map(|(&address, _)| CacheKey::Account(address))
            .collect();
        self.cache.write(entries(merged))?;
        self.cache.remove(&wiped);
        Ok(())
    }
}

/// Serves the EVM's reads, so the persistent store is the block state's physical_store.
impl<const N: usize> DatabaseRef for PersistentStore<'_, N> {
    type Error = StoreError;

    /// Reads the account unless it is absent or removed.
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, StoreError> {
        let info = self.get_with(&CacheKey::Account(address), |value| match value {
            CacheVal::Account(account)
                if !(account.is_selfdestructed()
                    || (account.is_loaded_as_not_existing() && !account.is_created())) =>
            {
                Some(account.info.clone())
            }
            _ => None,
        })?;
        Ok(info.flatten())
    }

    /// Reads the code by its hash; missing code reads as empty.
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, StoreError> {
        let code = self.get_with(&CacheKey::Code(code_hash), |value| match value {
            CacheVal::Code(code) => code.clone(),
            _ => Bytecode::default(),
        })?;
        Ok(code.unwrap_or_default())
    }

    /// Reads the slot: a removed account's slots read as zero, a created one's from the cache
    /// only. A Set's slot reads from the whole Set.
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, StoreError> {
        let status = self.get_with(&CacheKey::Account(address), |value| match value {
            CacheVal::Account(account) => (
                account.is_selfdestructed()
                    || (account.is_loaded_as_not_existing() && !account.is_created()),
                account.is_created(),
            ),
            _ => (false, false),
        })?;
        let (removed, created) = status.unwrap_or_default();
        if removed {
            return Ok(U256::ZERO);
        }
        let key = match set::KeyCodec::decode(index) {
            Some((set, ..)) => CacheKey::Set(address, set),
            None => CacheKey::Slot(address, index),
        };
        let read = |value: &CacheVal| match value {
            CacheVal::Set(slots) => slots.get(&index).map(|slot| slot.present_value),
            CacheVal::Slot(slot) => Some(slot.present_value),
            _ => None,
        };
        let value = match created {
            true => self.shards[self.index(&key)]
                .read()
                .unwrap()
                .get(&key)
                .and_then(read),
            false => self.get_with(&key, read)?.flatten(),
        };
        Ok(value.unwrap_or_default())
    }

    /// Reads the block hash.
    fn block_hash_ref(&self, number: u64) -> Result<B256, StoreError> {
        let hash = self.get_with(&CacheKey::BlockHash(number), |value| match value {
            CacheVal::BlockHash(hash) => *hash,
            _ => B256::ZERO,
        })?;
        Ok(hash.unwrap_or_default())
    }
}

/// Loads the persistent store's entries from MDBX's format: Sets are expanded from the store's
/// list into the indexed format, any other key read as is.
#[derive(Debug, Default)]
pub struct Expand;

/// Expands a Set as the cache loads it.
impl Load<CacheKey, CacheVal> for Expand {
    /// Loads `key`, expanding a Set from its list.
    fn load(
        physical_store: &dyn PhysicalStore<CacheKey, CacheVal>,
        key: &CacheKey,
    ) -> Result<Option<CacheVal>, StoreError> {
        let CacheKey::Set(owner, set) = *key else {
            return physical_store.get(key);
        };
        let mut expanding = Expanding {
            physical_store,
            slots: HashMap::default(),
        };
        set::Converter::expand(&mut expanding, owner, set::KeyCodec::length(set))?;
        Ok(Some(CacheVal::Set(expanding.slots)))
    }
}

/// Expands one Set from the store's list into a map, which keeps only the converted slots.
struct Expanding<'a> {
    /// The store the list is read from.
    physical_store: &'a dyn PhysicalStore<CacheKey, CacheVal>,
    /// The Set's slots, which become its cache entry.
    slots: HashMap<U256, EvmStorageSlot>,
}

/// The slot access `Converter::expand` runs on.
impl SlotStore for Expanding<'_> {
    type Error = StoreError;

    /// Reads the slot from the map, or as a plain slot from the store.
    fn read(&mut self, owner: Address, key: U256) -> Result<U256, StoreError> {
        if let Some(slot) = self.slots.get(&key) {
            return Ok(slot.present_value);
        }
        Ok(match self.physical_store.get(&CacheKey::Slot(owner, key))? {
            Some(CacheVal::Slot(slot)) => slot.present_value,
            _ => U256::ZERO,
        })
    }

    /// Writes the converted slot into the map.
    fn write(&mut self, _owner: Address, key: U256, value: U256) -> Result<(), StoreError> {
        self.slots.entry(key).or_default().present_value = value;
        Ok(())
    }
}

/// Lets a `StateDb` serve as the persistent store's physical_store, as in tests.
impl<DB, Store> PhysicalStore<CacheKey, CacheVal> for StateDb<DB, Store>
where
    DB: DatabaseRef + Send + Sync,
    DB::Error: Debug,
    Store: OverlayRead + Send + Sync,
{
    /// Loads the key from this state.
    fn get(&self, key: &CacheKey) -> Result<Option<CacheVal>, StoreError> {
        load(self, key)
    }
}

/// Loads a plain key from `db`. A store holds no Set entries: the cache expands Sets itself.
pub(crate) fn load<DB: DatabaseRef>(db: &DB, key: &CacheKey) -> Result<Option<CacheVal>, StoreError>
where
    DB::Error: Debug,
{
    let error = |error: DB::Error| StoreError::Database(format!("{error:?}"));
    Ok(match *key {
        CacheKey::Account(address) => db
            .basic_ref(address)
            .map_err(error)?
            .map(|info| CacheVal::Account(Account::from(info))),
        CacheKey::Slot(address, key) => {
            let value = db.storage_ref(address, key).map_err(error)?;
            Some(CacheVal::Slot(EvmStorageSlot::new(
                value,
                TransactionId::ZERO,
            )))
        }
        CacheKey::Set(..) => None,
        CacheKey::Code(hash) => Some(CacheVal::Code(db.code_by_hash_ref(hash).map_err(error)?)),
        CacheKey::BlockHash(number) => Some(CacheVal::BlockHash(
            db.block_hash_ref(number).map_err(error)?,
        )),
    })
}

#[cfg(test)]
#[path = "tests/cache.rs"]
mod tests;
