use std::hash::Hash;

use quick_cache::{Lifecycle, UnitWeighter, Weighter, unsync};

use crate::store::cache::{CachedStore, Load, Plain};
use crate::store::traits::StoreError;

/// Applies committed updates to the entries of a `CachedStore`.
/// It wraps the bare cache (see `CachedStore::into_cache`), so it never holds the fallback borrow.
pub struct CachedStoreWriter<
    'a,
    K,
    V,
    We = UnitWeighter,
    L = unsync::DefaultLifecycle<K, V>,
    Lo = Plain,
    const N: usize = 64,
> {
    cache: CachedStore<'a, K, V, We, L, Lo, N>,
}

impl<'a, K, V, We, L, Lo, const N: usize> CachedStoreWriter<'a, K, V, We, L, Lo, N> {
    /// Wraps the bare cache.
    pub fn new(cache: CachedStore<'a, K, V, We, L, Lo, N>) -> Self {
        Self { cache }
    }

    /// Returns the bare cache.
    pub fn into_inner(self) -> CachedStore<'a, K, V, We, L, Lo, N> {
        self.cache
    }
}

impl<K, V, We, L, Lo, const N: usize> CachedStoreWriter<'_, K, V, We, L, Lo, N>
where
    K: Eq + Hash + Send,
    V: Send,
    We: Weighter<K, V> + Send,
    L: Lifecycle<K, V> + Send,
    L::RequestState: Send,
    Lo: Load<K, V>,
{
    /// Writes `updates` into the cache without locks, each replacing any cached value.
    pub fn flush(&mut self, updates: Vec<(K, V)>) -> Result<(), StoreError> {
        self.cache.flush(updates, |current, value| match current {
            Some(current) => {
                *current = value;
                None
            }
            None => Some(value),
        });
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/writer.rs"]
mod tests;
