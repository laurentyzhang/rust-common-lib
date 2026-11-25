pub use super::error::StoreError;

pub trait Committer<K, V> {
    fn buffer(&mut self, id: K, state: V);
    /// Imports the accepted states and drops the rejected ones.
    fn stage(&mut self, accepted: Vec<K>) -> Result<(), StoreError>;
    /// Commit accepted states and clear the buffer on success.
    fn flush(&mut self) -> Result<(), StoreError>;
}

pub trait FallbackStore<K, V>: Send + Sync {
    fn contains_key(&self, key: &K) -> Result<bool, StoreError> {
        self.get_untracked(key).map(|value| value.is_some())
    }

    fn get(&self, key: &K) -> Result<Option<V>, StoreError>;

    // This is useful when one VM cache serves as the fallback for another,
    // as in nested VM cache scenarios. Without this method, an access may
    // leave tracked state in the parent store, which is not always desirable.
    // For other fallback stores, it is identical to `get` by default.
    fn get_untracked(&self, key: &K) -> Result<Option<V>, StoreError> {
        self.get(key)
    }
}
