use super::{EvmStateCommitter, JobResult};
use crate::planner::resolver::accepted_ids;
use crate::store::traits::Committer;
use alloy_evm::revm::state::EvmState;

/// Raw outcomes and the state committed by an execution layer.
pub struct StateOutcome<R, C> {
    pub raw: R,
    pub committed: C,
}

impl<R, H> StateOutcome<Vec<JobResult<R, H>>, EvmStateCommitter> {
    pub(crate) fn finalize(mut self) -> StateOutcome<Vec<JobResult<R, H>>, EvmState> {
        self.committed.stage(accepted_ids(
            self.raw.iter().filter_map(|raw| raw.verdict.as_ref()),
        ));
        self.committed.flush();
        StateOutcome {
            raw: self.raw,
            committed: self.committed.into_inner(),
        }
    }
}

#[cfg(test)]
#[path = "tests/state_outcome_tests.rs"]
mod tests;
