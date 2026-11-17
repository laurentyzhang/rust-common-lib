pub mod checkers;
pub mod detector;
mod metadata;
mod selection;
pub mod verdict;

pub use detector::ConflictDetector;
pub use verdict::{CommitDecision, CommitVerdict, accepted_ids, rejected_ids};

use crate::crdt::state::{Status, Tracked};
use crate::execution::output::ExecutionOutput;
use crate::planner::scheduler::ID;

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

/// Converts signed, unsigned, and state compatibility errors into numeric errors.
impl From<crate::crdt::state::CrdtError> for NumericError {
    fn from(error: crate::crdt::state::CrdtError) -> Self {
        match error {
            crate::crdt::state::CrdtError::I256(error)
            | crate::crdt::state::CrdtError::U256(error)
            | crate::crdt::state::CrdtError::U64(error) => error.into(),
            crate::crdt::state::CrdtError::CannotAddDeltaToMissingValue => {
                Self::CannotAddDeltaToMissingValue
            }
            crate::crdt::state::CrdtError::TypeMismatch => Self::TypeMismatch,
        }
    }
}

/// Incompatible accesses to the same key, with diagnostic details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessConflict {
    CreationConflict(String),
    ExistenceConflict(String),
    ReadConflict(String),
    DeltaConflict(String),
    RemoveConflict(String),
}

/// Describes an access conflict, numeric constraint failure, or rejected dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason<K = ()> {
    NumericError(K, NumericError),
    AccessConflict(K, AccessConflict),
    /// A rejected job that this job depends on.
    Dependency(ID),
    None,
}

/// Groups access conflicts for a single transaction.
#[derive(Clone)]
pub struct TransactionConflictReport {
    pub tx_id: u64,
    pub access_conflicts: Vec<AccessConflict>,
}

/// Affected job, related peers, and keyed reason used for selection and reporting.
pub struct Diagnostic<K> {
    pub id: ID,
    pub peers: Vec<ID>,
    pub reason: Reason<K>,
}

/// Collects execution records and returns complete per-job verdicts for a generation.
pub trait Checker<K> {
    fn import(&mut self, records: Vec<ExecutionOutput<K, Tracked<Status, Status>>>);
    fn check(&mut self) -> Vec<CommitVerdict<K>>;
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
