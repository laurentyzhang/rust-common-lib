pub mod checkers;
pub mod detector;
mod selection;
pub mod verdict;

pub use detector::ConflictDetector;
pub use verdict::{CommitDecision, CommitVerdict, accepted_ids, rejected_ids};

use crate::crdt::state::{CrdtError, Status, Tracked};
use crate::execution::output::ExecutionOutput;
use crate::planner::scheduler::ID;

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
    GasLimitExceeded,
    NumericError(K, CrdtError),
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
#[derive(Clone)]
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
#[path = "tests/resolver.rs"]
mod tests;
