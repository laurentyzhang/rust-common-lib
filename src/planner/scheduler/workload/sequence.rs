use super::super::sender::SenderNonceOffset;
use super::Job;
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

use crate::eu::{ArcologyEvmFactory, JobOutcome, ReadRecorder, StateDb};

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
    pub(in crate::planner::scheduler) fn new(jobs: Vec<Job<Tx>>) -> Self {
        Self {
            jobs,
            nonce_offsets: Vec::new(),
        }
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
    /// Executes each job in a fresh EVM, carrying completed changes forward locally.
    /// Retains each job's result; errors leave the sequence state unchanged.
    pub fn run<F, DB>(
        self,
        factory: &F,
        fallback: DB,
        env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
        ctx: OpBlockExecutionCtx,
    ) -> Result<Vec<JobOutcome<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>, Error>
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        F::Receipt: Clone,
        DB: DatabaseRef + std::fmt::Debug,
        Tx: ExecutableTxParts<<F::EvmFactory as EvmFactory>::Tx, F::Transaction>,
    {
        if self.jobs.is_empty() {
            return Ok(Vec::new());
        }

        let initialized = self.initialize_state(&fallback)?;

        // Keep completed transaction changes local for subsequent jobs.
        let mut seq_db = StateDb::with_fallback(initialized, Some(fallback));

        let mut results = Vec::with_capacity(self.jobs.len());
        for job in self.jobs {
            let result = {
                let job_db = StateDb::with_fallback(EvmState::default(), Some(&seq_db));
                let evm = factory.evm_factory().create_evm_with_inspector(
                    job_db,
                    env.clone(),
                    ReadRecorder::new(job.id.global),
                );
                let mut executor = factory.create_executor(evm, ctx.clone());
                job.run(&mut executor)
            };

            if let Ok(result_and_state) = &result.result_and_state {
                seq_db.commit(result_and_state.state.clone());
            }
            results.push(result);
        }
        Ok(results)
    }

    /// Loads sender accounts from the fallback and advances their local nonces
    /// by earlier same-sender transactions in this generation. Leaves the fallback unchanged.
    fn initialize_state<DB: DatabaseRef>(&self, fallback: &DB) -> Result<EvmState, Error> {
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
