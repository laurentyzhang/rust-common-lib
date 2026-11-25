use super::{Job, JobResult, ReceiptStatus, TransactionGasLimit};
use crate::planner::scheduler::sender::SenderNonceOffset;
use crate::{
    crdt::state,
    planner::resolver::{CommitDecision, CommitVerdict, Reason},
};
use alloy_evm::{
    EvmEnv, EvmFactory,
    block::{BlockExecutorFactory, ExecutableTxParts},
    revm::{
        DatabaseCommit, DatabaseRef,
        state::{Account, EvmState},
    },
};
use alloy_op_evm::{OpBlockExecutionCtx, post_exec::PostExecEvmFactoryAdapter};
use alloy_primitives::Address;
use std::io::Error;

use crate::execution::evm::state::StateOverlay;
use crate::execution::{ArcologyEvmFactory, DeltaMerger, EvmStateCommitter, Recorder, StateDb, scalar};

/// Transactions in one sequence execute in order; sequences in a generation may run in parallel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobSequence<Tx> {
    pub jobs: Vec<Job<Tx>>,
    /// One entry per sender in this sequence, ordered by sender address.
    /// Offsets count that sender's earlier transactions within the generation.
    pub nonce_offsets: Vec<SenderNonceOffset>,
}

impl<Tx> JobSequence<Tx> {
    /// Keeps the supplied transaction order and leaves nonce offsets empty.
    /// The generation constructor calculates offsets once its layout is fixed.
    pub(crate) fn new(jobs: Vec<Job<Tx>>) -> Self {
        Self {
            jobs,
            nonce_offsets: Vec::new(),
        }
    }

    /// Preserves direct conflicts and rejects downstream jobs due to the first rejection.
    pub(crate) fn assign_verdicts<'a, K: 'a>(
        verdicts: impl IntoIterator<Item = &'a mut CommitVerdict<K>>,
    ) {
        let mut dependency = None;
        for verdict in verdicts {
            if matches!(
                verdict.decision,
                CommitDecision::Pending | CommitDecision::Dropped
            ) {
                continue;
            }
            if let Some(cause) = dependency {
                verdict.decision = CommitDecision::Rejected;
                verdict
                    .conflicts
                    .retain(|(_, reason)| !matches!(reason, Reason::Dependency(_)));
                if !verdict.conflicts.iter().any(|(_, reason)| {
                    matches!(
                        reason,
                        Reason::AccessConflict(_, _) | Reason::NumericError(_, _)
                    )
                }) {
                    verdict
                        .conflicts
                        .push((verdict.id, Reason::Dependency(cause)));
                }
            } else if verdict.decision == CommitDecision::Rejected {
                dependency = Some(verdict.id);
            }
        }
    }

    pub(crate) fn is_all_accepted<R, H>(raws: &[JobResult<R, H>]) -> bool {
        raws.iter()
            .all(|result| result.verdict.decision == CommitDecision::Accepted)
    }
}

impl<Tx> Default for JobSequence<Tx> {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            nonce_offsets: Vec::new(),
        }
    }
}

impl<Tx> JobSequence<Tx> {
    /// Returns the gas reserved by the sequence's declared transaction limits.
    pub(super) fn gas_limit(&self) -> Result<u64, Error>
    where
        Tx: TransactionGasLimit,
    {
        self.jobs
            .iter()
            .try_fold(0u64, |gas, job| gas.checked_add(job.tx.gas_limit()))
            .ok_or_else(|| Error::other("declared sequence gas exceeds u64"))
    }

    /// Executes jobs in order within their reserved sequence gas budget.
    /// Returns each job's raw outcome and the committed sequence state.
    /// Errors leave the sequence state unchanged.
    pub fn run<F, Fallback>(
        self,
        factory: &F,
        fallback: &Fallback,
        env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
        ctx: OpBlockExecutionCtx,
    ) -> Result<
        (
            Vec<JobResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
            EvmState,
        ),
        Error,
    >
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        F::Receipt: Clone + ReceiptStatus,
        Fallback: DatabaseRef + Sync + std::fmt::Debug,
        Tx: TransactionGasLimit
            + ExecutableTxParts<<F::EvmFactory as EvmFactory>::Tx, F::Transaction>,
    {
        let gas_limit = self.gas_limit()?;
        if gas_limit > env.block_env.gas_limit {
            return Err(Error::other(format!(
                "declared sequence gas {gas_limit} exceeds block gas limit {}",
                env.block_env.gas_limit
            )));
        }
        let (mut raws, mut committed) = self.execute(factory, fallback, env, ctx, gas_limit)?;
        for raw in &mut raws {
            if raw.gas_used().is_some() {
                raw.verdict.decision = CommitDecision::Accepted;
            }
        }
        if !Self::is_all_accepted(&raws) {
            committed = Self::rebuild_sequence_state(&mut raws, fallback)?;
        }
        Ok((raws, committed))
    }

    /// Executes jobs in a shared sequence gas pool and carries their state forward locally.
    pub(super) fn execute<F, Fallback>(
        self,
        factory: &F,
        fallback: &Fallback,
        env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
        ctx: OpBlockExecutionCtx,
        mut remaining_gas: u64,
    ) -> Result<
        (
            Vec<JobResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
            EvmState,
        ),
        Error,
    >
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        F::Receipt: Clone,
        Fallback: DatabaseRef + Sync + std::fmt::Debug,
        Tx: TransactionGasLimit
            + ExecutableTxParts<<F::EvmFactory as EvmFactory>::Tx, F::Transaction>,
    {
        if self.jobs.is_empty() {
            return Ok((Vec::new(), EvmState::default()));
        }

        let initialized = self.offset_nonce(fallback)?;

        // Keep completed transaction changes local for subsequent jobs.
        let mut seq_db = StateDb::with_fallback(
            StateOverlay::new(initialized, StateOverlay::copy_balance_nonce),
            Some(fallback),
        );

        let mut raws = Vec::with_capacity(self.jobs.len());
        for job in self.jobs {
            let gas_limit = job.tx.gas_limit();
            if gas_limit > remaining_gas {
                return Err(Error::other(format!(
                    "declared gas {gas_limit} for job {:?} exceeds remaining sequence gas {remaining_gas}",
                    job.id
                )));
            }
            let result = {
                let evm = factory.evm_factory().create_evm_with_inspector(
                    &mut seq_db,
                    env.clone(),
                    Recorder::new(job.id.global),
                );
                let mut executor = factory.create_executor(evm, ctx.clone());
                job.run(&mut executor)
            };

            if let Some(error) = seq_db.commit_error() {
                return Err(Error::other(format!(
                    "failed to commit job {:?}. {error}",
                    result.id
                )));
            }
            remaining_gas -= result.gas_used().unwrap_or_default();
            raws.push(result);
        }

        Ok((raws, seq_db.into_inner()))
    }

    /// Applies job transitions in order and returns the sequence state ready to commit.
    ///
    /// Each job supplies its transitions through `result_and_state()`, which owns
    /// verdict handling and reports pending verdicts or execution errors. Account
    /// and API-counter transitions are represented by their original and resulting
    /// values, so their deltas can be applied to the rebuilt sequence state.
    ///
    /// `fallback` supplies the starting account and storage values.
    pub(crate) fn rebuild_sequence_state<R: ReceiptStatus, H: Clone, DB: DatabaseRef>(
        raws: &mut [JobResult<R, H>],
        fallback: &DB,
    ) -> Result<EvmState, Error> {
        let overlay =
            StateOverlay::new(EvmState::default(), StateOverlay::add_balance_nonce_deltas);
        let mut db = StateDb::with_fallback(overlay, Some(fallback));
        // Jobs after a non-accepted one are rejected, so the accepted jobs form a prefix and their
        // cumulative Set logs hold only accepted changes.
        debug_assert!(raws.windows(2).all(|pair| {
            pair[0].verdict.decision == CommitDecision::Accepted
                || pair[1].verdict.decision != CommitDecision::Accepted
        }));
        for result in raws.iter_mut() {
            let executed = result
                .result_and_state()
                .as_ref()
                .map_err(|error| Error::other(error.to_string()))?;
            // Scalar deltas add up across jobs; the other slots take the later job's values.
            let mut state = executed.state.clone();
            let scalars =
                EvmStateCommitter::split(&mut state, |key| scalar::KeyCodec::decode(key).is_some());
            db.commit(state);
            if let Some(error) = db.commit_error() {
                return Err(Error::other(error.to_string()));
            }
            for (owner, slots) in scalars {
                scalar::Merger::merge(&mut db, owner, &slots).map_err(Error::other)?;
            }
        }
        Ok(db.into_inner())
    }

    /// Loads sender accounts from the fallback and advances their local nonces
    /// by earlier same-sender transactions in this generation. Leaves the fallback unchanged.
    fn offset_nonce<DB: DatabaseRef>(&self, fallback: &DB) -> Result<EvmState, Error> {
        let mut state = EvmState::default();
        for entry in &self.nonce_offsets {
            if entry.offset == 0 {
                continue;
            }
            let address = Address::from(entry.sender);
            let mut info = fallback
                .basic_ref(address)
                .map_err(Error::other)?
                .unwrap_or_default();
            info.nonce = info
                .nonce
                .checked_add(entry.offset)
                .ok_or_else(|| Error::other("sequence sender nonce overflow"))?;
            state.insert(address, Account::from(info).with_touched_mark());
        }
        Ok(state)
    }
}

#[cfg(test)]
#[path = "../tests/sequence.rs"]
mod tests;
