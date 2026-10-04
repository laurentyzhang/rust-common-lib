use crate::crdt::state::StateError;

pub trait FallbackStore<'a, K, V>: Send + Sync {
    fn contains_key(&self, key: &K) -> bool; //If the key exists locally.
    fn get(&self, key: &K) -> Option<&V>;

    // This is useful when one VM cache serves as the fallback for another,
    // as in nested VM cache scenarios. Without this method, an access may
    // leave tracked state in the parent store, which is not always desirable.
    // For other fallback stores, it is identical to `get` by default.
    fn get_untracked(&self, key: &K) -> Option<&V> {
        self.get(key)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    State(StateError),
    SetNoneToValue,
    DeleteNonexistingEntry,
    ValueCannotBeRecreated,
    ValueCannotBeStripped,
    NotFound,
    EntryNotFound,
}

impl From<StateError> for StoreError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}
