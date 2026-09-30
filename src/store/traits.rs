use crate::crdt::state::StateError;

pub trait FallbackStore<'a, K, V>: Send + Sync {
    fn contains_key(&self, key: &K) -> bool; //If the key exists locally.
    fn get(&self, key: &K) -> Option<&V>;

    fn get_raw(&self, key: &K) -> Option<&V> {
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
