use crate::crdt::state::CrdtError;

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    State(CrdtError),
    Transition(String),
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
        match self {
            Self::Transition(message) => f.write_str(message),
            _ => write!(f, "{self:?}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<crate::common::codec::CodecError> for StoreError {
    /// Reports a slot codec failure as a store codec error.
    fn from(error: crate::common::codec::CodecError) -> Self {
        Self::Codec(error.to_string())
    }
}

impl From<CrdtError> for StoreError {
    fn from(error: CrdtError) -> Self {
        Self::State(error)
    }
}
