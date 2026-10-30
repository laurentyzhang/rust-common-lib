use super::super::sender::SenderNonceOffset;
use super::Job;
use alloy_evm::{
    EvmEnv, EvmFactory,
    block::{
        BlockExecutionError, BlockExecutor, BlockExecutorFactory, ExecutableTx, StateDB, TxResult,
    },
};
use alloy_op_evm::{OpBlockExecutionCtx, post_exec::PostExecEvmFactoryAdapter};

use crate::eu::{ArcologyEvmFactory, JobOutcome, ReadRecorder};

/// Transactions in one sequence execute in order; sequences in a generation may run in parallel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobSequence<Tx, DB = ()> {
    pub db: DB,
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
            db: (),
            jobs,
            nonce_offsets: Vec::new(),
        }
    }
}

impl<Tx, DB: Default> Default for JobSequence<Tx, DB> {
    fn default() -> Self {
        Self {
            db: DB::default(),
            jobs: Vec::new(),
            nonce_offsets: Vec::new(),
        }
    }
}

impl<Tx, DB: StateDB> JobSequence<Tx, DB> {
    /// Executes jobs in order against this sequence's private database.
    pub fn run<F>(
        self,
        factory: &F,
        env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
        ctx: OpBlockExecutionCtx,
        _nonce_offsets: Vec<SenderNonceOffset>,
    ) -> Result<
        Vec<JobOutcome<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
        BlockExecutionError,
    >
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        F::Receipt: Clone,
        Tx: for<'a> ExecutableTx<F::Executor<'a, DB, ReadRecorder>>,
    {
        let Some(first_id) = self.jobs.first().map(|job| job.id.global) else {
            return Ok(Vec::new());
        };

        let evm = factory.evm_factory().create_evm_with_inspector(
            self.db,
            env,
            ReadRecorder::new(first_id),
        );

        let mut executor = factory.create_executor(evm, ctx);
        let mut results = Vec::with_capacity(self.jobs.len());
        for job in self.jobs {
            let id = job.id;
            // Reset reads before each job, including jobs following a failed execution.
            executor.evm_mut().reset_reads(id.global);
            let mut result_and_state = None;
            executor.execute_transaction_with_result_closure(job.tx, |result| {
                result_and_state = Some(result.result().clone());
            })?;
            let receipt = executor
                .receipts()
                .last()
                .cloned()
                .expect("a committed transaction has a receipt");
            let merged_reads = executor.evm_mut().drain_reads();
            results.push(JobOutcome::new(
                id,
                result_and_state.expect("a successful execution has a result"),
                receipt,
                merged_reads,
            ));
        }
        Ok(results)
    }
}
