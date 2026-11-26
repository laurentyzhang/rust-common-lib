use std::collections::BTreeSet;

use alloy_evm::revm::{DatabaseRef, state::EvmState};
use alloy_primitives::{Address, U256};

use crate::execution::evm::api_store::ApiStore;
use crate::execution::evm::state::{StateDb, StateOverlay, sharded::Sharded};
use crate::execution::{FormatConverter, set};
use crate::store::traits::{FallbackStore, StoreError};

pub struct CachedStore<'a, K, V> {
    pub(super) cache: quick_cache::sync::Cache<K, V>,
    fallback: Option<&'a dyn FallbackStore<K, V>>,
}

impl<'a, K, V> CachedStore<'a, K, V>
where
    K: Eq + std::hash::Hash,
    V: Clone,
{
    pub fn new(capacity: usize, fallback: Option<&'a dyn FallbackStore<K, V>>) -> Self {
        Self {
            cache: quick_cache::sync::Cache::new(capacity),
            fallback,
        }
    }
}

impl<'a, K, V> CachedStore<'a, K, V> {
    pub fn into_cache(self) -> quick_cache::sync::Cache<K, V> {
        self.cache
    }

    pub fn from_cache(
        cache: quick_cache::sync::Cache<K, V>,
        fallback: Option<&'a dyn FallbackStore<K, V>>,
    ) -> Self {
        Self { cache, fallback }
    }
}

impl<K, V> FallbackStore<K, V> for CachedStore<'_, K, V>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    fn contains_key(&self, key: &K) -> Result<bool, StoreError> {
        if self.cache.contains_key(key) {
            return Ok(true);
        }
        match self.fallback {
            Some(fallback) => fallback.contains_key(key),
            None => Ok(false),
        }
    }

    fn get(&self, key: &K) -> Result<Option<V>, StoreError> {
        if let Some(value) = self.cache.get(key) {
            return Ok(Some(value));
        }

        let Some(fallback) = self.fallback else {
            return Ok(None);
        };
        let Some(value) = fallback.get(key)? else {
            return Ok(None);
        };
        self.cache.insert(key.clone(), value.clone());
        Ok(Some(value))
    }

    fn get_untracked(&self, key: &K) -> Result<Option<V>, StoreError> {
        self.get(key)
    }
}

// Sets are in the storage format only in the store; the cache and every layer above it hold the
// execution format. Both conversions happen here, between the cache and the store.

/// Converts the Set `key` belongs to into the execution format in `cache` as it is loaded from the
/// store, loading missing slots with `load`. Skipped once converted, or while the account has no
/// entry, so a written slot never stands in for the account; a failed conversion leaves the Set
/// as loaded.
pub(crate) fn to_execution<const N: usize>(
    cache: &Sharded<N>,
    address: Address,
    key: U256,
    load: impl Fn(U256) -> Option<U256>,
) {
    let Some((set, ..)) = set::KeyCodec::decode(key) else {
        return;
    };
    let converted = cache.read_slot(address, set::KeyCodec::indexed(set), |slot| {
        slot.is_some_and(|slot| !slot.present_value.is_zero())
    });
    if converted || cache.read(address, |account| account.is_none()) {
        return;
    }
    let _ = set::Converter::to_execution_format(&mut Converting { cache, load }, address, set);
}

/// Store for converting a Set in the cache: reads from the cache or loads missing slots, writes
/// into the cache.
struct Converting<'a, const N: usize, F> {
    cache: &'a Sharded<N>,
    load: F,
}

impl<const N: usize, F: Fn(U256) -> Option<U256>> ApiStore for Converting<'_, N, F> {
    type Error = StoreError;

    /// Reads the slot from the cache, or loads it.
    fn read(&mut self, owner: Address, key: U256) -> Result<U256, StoreError> {
        match self
            .cache
            .read_slot(owner, key, |slot| slot.map(|slot| slot.present_value))
        {
            Some(value) => Ok(value),
            None => (self.load)(key).ok_or_else(|| {
                StoreError::Database(format!(
                    "failed to load account {owner} slot {key:#x} for Set conversion"
                ))
            }),
        }
    }

    /// Writes the converted slot into the cache.
    fn write(&mut self, owner: Address, key: U256, value: U256) -> Result<(), StoreError> {
        self.cache
            .write_slot(owner, key, |slot| slot.present_value = value);
        Ok(())
    }
}

/// Converts the touched Sets in `changes` into the storage format for the store, reading their
/// execution format from `changes` and then `cache`; returns the changes to persist, still open
/// for further commits.
pub(crate) fn to_storage<DB: DatabaseRef>(
    changes: EvmState,
    cache: &DB,
) -> Result<StateDb<&DB>, StoreError> {
    let mut db = StateDb::with_fallback(
        StateOverlay::new(changes, StateOverlay::copy_balance_nonce),
        Some(cache),
    );
    let sets: Vec<(Address, Vec<U256>)> = db
        .inner()
        .iter()
        .map(|(&owner, account)| {
            let indexed: BTreeSet<U256> = account
                .storage
                .keys()
                .filter_map(|&key| set::KeyCodec::decode(key))
                .map(|(set, ..)| set::KeyCodec::indexed(set))
                .collect();
            (owner, indexed.into_iter().collect())
        })
        .collect();
    for (owner, keys) in sets {
        set::Converter::to_storage_format(&mut db, owner, &keys)?;
    }
    Ok(db)
}

#[cfg(test)]
#[path = "tests/cache.rs"]
mod tests;
