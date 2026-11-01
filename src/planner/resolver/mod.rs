pub mod checkers;
pub mod commit_plan;
pub mod conflict_checker;
pub mod reason;

pub use commit_plan::{CommitPlan, ConflictResult, RejectPlan};
pub use conflict_checker::ConflictChecker;

use crate::crdt::state::{Status, Tracked};
use crate::eu::output::ExecutionOutput;
use reason::Reason;

pub type Rejected<K> = (ExecutionOutput<K, Tracked<Status, Status>>, Reason);

/// Collects execution records and reports constraint violations for a generation.
pub trait Checker<K> {
    fn import(&mut self, records: Vec<ExecutionOutput<K, Tracked<Status, Status>>>);

    fn check(&mut self) -> ConflictResult<K>;
}

#[cfg(test)]
mod tests;
