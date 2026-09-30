#[derive(Debug, PartialEq, Eq, Clone)]
pub enum NumericError {
    InvalidLimits(String),
    BelowLowerLimit(String),
    AboveUpperLimit(String),
    Overflow(String),
    Underflow(String),
    CannotAddDeltaToMissingValue,
    TypeMismatch,
}

impl From<crate::crdt::state::NumericError> for NumericError {
    fn from(error: crate::crdt::state::NumericError) -> Self {
        match error {
            crate::crdt::state::NumericError::InvalidLimits(message) => {
                Self::InvalidLimits(message)
            }
            crate::crdt::state::NumericError::BelowLowerLimit(message) => {
                Self::BelowLowerLimit(message)
            }
            crate::crdt::state::NumericError::AboveUpperLimit(message) => {
                Self::AboveUpperLimit(message)
            }
            crate::crdt::state::NumericError::Overflow(message) => Self::Overflow(message),
            crate::crdt::state::NumericError::Underflow(message) => Self::Underflow(message),
        }
    }
}

impl From<crate::crdt::state::StateError> for Reason {
    fn from(error: crate::crdt::state::StateError) -> Self {
        let error = match error {
            crate::crdt::state::StateError::I64(error)
            | crate::crdt::state::StateError::U64(error)
            | crate::crdt::state::StateError::U256(error) => error.into(),
            crate::crdt::state::StateError::CannotAddDeltaToMissingValue => {
                NumericError::CannotAddDeltaToMissingValue
            }
            crate::crdt::state::StateError::TypeMismatch => NumericError::TypeMismatch,
        };

        Self::NumericError(error)
    }
}

#[derive(Clone, Debug)]
pub enum AccessConflict {
    ReadWrite(String),
    ReadDelete(String),
    DeltaDelete(String),
    ConcurrentDeltaWritesOnNonCommutative(String),
}

#[derive(Debug, Clone)]
pub enum Reason {
    NumericError(NumericError),
    AccessConflict(AccessConflict),
    None,
}

#[derive(Clone)]
pub struct TransactionConflictReport {
    pub tx_id: u64,
    pub access_conflicts: Vec<AccessConflict>,
}

impl AccessConflict {
    pub(crate) fn with_key<K: std::fmt::Debug>(self, key: &K) -> Self {
        match self {
            Self::ReadWrite(message) => Self::ReadWrite(format!("{message}; key={key:?}")),
            Self::ReadDelete(message) => Self::ReadDelete(format!("{message}; key={key:?}")),
            Self::DeltaDelete(message) => Self::DeltaDelete(format!("{message}; key={key:?}")),
            Self::ConcurrentDeltaWritesOnNonCommutative(message) => {
                Self::ConcurrentDeltaWritesOnNonCommutative(format!("{message}; key={key:?}"))
            }
        }
    }
}
