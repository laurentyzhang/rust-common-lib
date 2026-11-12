use super::{EvmStateCommitter, JobResult};
use crate::planner::resolver::CommitDecision;
use crate::store::traits::Committer;
use alloy_evm::revm::{DatabaseRef, state::EvmState};
use std::io::Error;

/// Raw outcomes and the state committed by an execution layer.
pub struct StateOutcome<R, C> {
    pub raw: R,
    pub committed: C,
}

impl<R, H> StateOutcome<Vec<JobResult<R, H>>, EvmStateCommitter> {
    pub(crate) fn finalize<DB: DatabaseRef>(
        mut self,
        fallback: &DB,
    ) -> Result<StateOutcome<Vec<JobResult<R, H>>, EvmState>, Error> {
        for raw in &self.raw {
            // Replace state only for jobs explicitly rejected by conflict checking.
            if raw.verdict.decision != CommitDecision::Rejected {
                continue;
            }
            // Replace the rejected job's buffered state with its nonce and fee changes.
            self.committed.buffer(raw.id, raw.fee_nonce_only_state()?);
        }
        Ok(StateOutcome {
            raw: self.raw,
            committed: self.committed.commit(fallback)?,
        })
    }
}

#[cfg(test)]
#[path = "tests/state_outcome_tests.rs"]
mod tests;
