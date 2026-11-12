use super::{EvmStateCommitter, JobResult};
use crate::store::traits::Committer;
use alloy_evm::revm::{DatabaseRef, state::EvmState};
use std::io::Error;

/// Execution results and their associated state changes.
pub struct StateOutcome<R, C> {
    /// Job results, including execution results, receipts, and verdicts.
    pub raw: R,
    /// State changes: per-job states before finalization, or merged state afterward.
    pub committed: C,
}

impl<R, H: Clone> StateOutcome<Vec<JobResult<R, H>>, Vec<EvmState>> {
    pub(crate) fn finalize<DB: DatabaseRef>(
        mut self,
        fallback: &DB,
    ) -> Result<StateOutcome<Vec<JobResult<R, H>>, EvmState>, Error> {
        if self.raw.len() != self.committed.len() {
            return Err(Error::other("job and state counts do not match"));
        }
        let mut committer = EvmStateCommitter::new_sequential();
        for (raw, state) in self.raw.iter_mut().zip(self.committed) {
            let state = raw.state_for_commit(state)?;
            committer.buffer(raw.id, state);
        }
        Ok(StateOutcome {
            raw: self.raw,
            committed: committer.commit(fallback)?,
        })
    }
}

#[cfg(test)]
#[path = "tests/state_outcome_tests.rs"]
mod tests;
