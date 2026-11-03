pub mod checkers;
pub mod commit_plan;
pub mod detector;
mod metadata;

pub use commit_plan::{CONFLICT_META_ID, CommitPlan, ConflictResult, RejectPlan};
pub use detector::ConflictDetector;

use crate::crdt::state::{Status, Tracked};
use crate::eu::output::ExecutionOutput;

/// Numeric constraint or state failures encountered while accumulating deltas.
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

/// Preserves CRDT numeric error categories and diagnostic messages.
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

/// Converts signed, unsigned, and state compatibility errors into numeric rejections.
impl From<crate::crdt::state::StateError> for Reason {
    fn from(error: crate::crdt::state::StateError) -> Self {
        let error = match error {
            crate::crdt::state::StateError::I256(error)
            | crate::crdt::state::StateError::U256(error) => error.into(),
            crate::crdt::state::StateError::CannotAddDeltaToMissingValue => {
                NumericError::CannotAddDeltaToMissingValue
            }
            crate::crdt::state::StateError::TypeMismatch => NumericError::TypeMismatch,
        };

        Self::NumericError(error)
    }
}

/// Incompatible accesses to the same key, with diagnostic details.
#[derive(Clone, Debug)]
pub enum AccessConflict {
    CreationConflict(String),
    ExistenceConflict(String),
    ReadConflict(String),
    DeltaConflict(String),
    RemoveConflict(String),
    ConcurrentDeltaOnNonCommutative(String),
}

/// Explains why a resolver checker rejected an execution record.
#[derive(Debug, Clone)]
pub enum Reason {
    NumericError(NumericError),
    AccessConflict(AccessConflict),
    None,
}

/// Groups access conflicts for a single transaction.
#[derive(Clone)]
pub struct TransactionConflictReport {
    pub tx_id: u64,
    pub access_conflicts: Vec<AccessConflict>,
}

/// An execution record paired with its rejection reason.
pub type Violation<K> = (ExecutionOutput<K, Tracked<Status, Status>>, Reason);

/// Collects execution records and reports constraint violations for a generation.
pub trait Checker<K> {
    fn import(&mut self, records: Vec<ExecutionOutput<K, Tracked<Status, Status>>>);
    fn check(&mut self) -> ConflictResult<K>;
}

#[cfg(test)]
mod tests;
