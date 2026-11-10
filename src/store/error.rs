use crate::crdt::state::CrdtError;

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    State(CrdtError),
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

impl From<CrdtError> for StoreError {
    fn from(error: CrdtError) -> Self {
        Self::State(error)
    }
}
