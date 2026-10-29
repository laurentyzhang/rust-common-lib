use crate::crdt::state::StateError;
use crate::store::StoreError;

#[derive(Debug, PartialEq)]
pub enum Error {
    State(StateError),
    Store(StoreError),
}

impl From<StateError> for Error {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
