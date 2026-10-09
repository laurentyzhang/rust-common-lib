use crate::crdt::state::StateError;

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

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    State(StateError),
    SetNoneToValue,
    DeleteNonexistingEntry,
    ValueCannotBeRecreated,
    ValueCannotBeStripped,
    NotFound,
    EntryNotFound,
    Database(String),
    Codec(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for StoreError {}

impl From<StateError> for StoreError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}
