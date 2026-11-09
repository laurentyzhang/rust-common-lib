use super::{EvmStateCommitter, JobResult};
use crate::store::traits::Committer;
use alloy_evm::revm::{
    DatabaseRef,
    state::{Account, EvmState},
};
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
            if raw.verdict.as_ref().is_none_or(|verdict| verdict.accepted) {
                continue;
            }
            // Replace the rejected job's buffered state with its nonce and fee changes.
            self.committed.buffer(raw.id, nonce_and_fee(raw)?);
        }
        Ok(StateOutcome {
            raw: self.raw,
            committed: self.committed.commit(fallback)?,
        })
    }
}

/// Builds the state kept for a rejected job: the sender's nonce and fee, and the fee recipients' credits.
/// Application storage and code changes are left out so conflict txs can still pay for execution.
fn nonce_and_fee<R, H>(raw: &JobResult<R, H>) -> Result<EvmState, Error> {
    let mut state = EvmState::default();
    let Ok(executed) = &raw.result_and_state else {
        return Ok(state);
    };
    if let Some(sender) = executed.state.get(&raw.sender) {
        // Rebuild from pre-execution info, leaving application storage and code changes out.
        let mut account = Account::from(sender.original_info()).with_touched_mark();
        account.info.code = None;
        // Preserve the nonce consumed by execution.
        account.info.nonce = sender.info.nonce;
        // Charge fees against the original balance, excluding value transfers.
        account.info.balance = account
            .info
            .balance
            .checked_sub(raw.fee)
            .ok_or_else(|| Error::other("insufficient balance for rejected transaction fee"))?;
        state.insert(raw.sender, account);
    }

    // Pay fee recipients even though the job's application state was rejected.
    for &(recipient, amount) in &raw.fee_recipients {
        if amount.is_zero() {
            continue;
        }
        let original = executed
            .state
            .get(&recipient)
            .expect("a paid fee recipient is present in the executed state")
            .original_info();
        // Reuse the sender account when it is also a fee recipient, such as coinbase.
        let account = state
            .entry(recipient)
            .or_insert_with(|| Account::from(original).with_touched_mark());
        account.info.code = None;
        account.info.balance = account
            .info
            .balance
            .checked_add(amount)
            .ok_or_else(|| Error::other("fee recipient balance overflow"))?;
    }
    Ok(state)
}

#[cfg(test)]
#[path = "tests/state_outcome_tests.rs"]
mod tests;
