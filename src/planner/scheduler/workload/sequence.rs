use super::super::sender::SenderNonceOffset;
use super::Job;
use alloy_evm::{
    EvmEnv, EvmFactory,
    block::{BlockExecutionError, BlockExecutorFactory, ExecutableTx},
    revm::{
        DatabaseRef,
        state::{Account, EvmState},
    },
};
use alloy_op_evm::{OpBlockExecutionCtx, post_exec::PostExecEvmFactoryAdapter};
use alloy_primitives::Address;

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
    /// Executes jobs using local state over the supplied read-only fallback.
    pub fn run<F, DB>(
        self,
        factory: &F,
        fallback: DB,
        env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
        ctx: OpBlockExecutionCtx,
    ) -> Result<
        Vec<JobOutcome<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
        BlockExecutionError,
    >
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        F::Receipt: Clone,
        DB: DatabaseRef + std::fmt::Debug,
        Tx: for<'a> ExecutableTx<F::Executor<'a, StateDb<DB>, ReadRecorder>>,
    {
        let Some(first_id) = self.jobs.first().map(|job| job.id.global) else {
            return Ok(Vec::new());
        };

        let state = self.initialize_state(&fallback)?;
        let db = StateDb::with_fallback(state, Some(fallback));

        let evm =
            factory
                .evm_factory()
                .create_evm_with_inspector(db, env, ReadRecorder::new(first_id));

        let mut executor = factory.create_executor(evm, ctx);
        let mut results = Vec::with_capacity(self.jobs.len());
        for job in self.jobs {
            results.push(job.run(&mut executor)?);
        }
        Ok(results)
    }

    /// Initializes the local EVM state based on the nonce offsets.
    /// To compensate for incorrect initial nonce values in parallel execution.
    fn initialize_state<DB: DatabaseRef>(
        &self,
        fallback: &DB,
    ) -> Result<EvmState, BlockExecutionError> {
        let mut state = EvmState::default();
        for entry in &self.nonce_offsets {
            if entry.offset == 0 {
                continue;
            }
            let address = Address::from(entry.sender);
            let mut info = fallback
                .basic_ref(address)
                .map_err(|error| {
                    BlockExecutionError::msg(format!("sequence initialization failed: {error}"))
                })?
                .unwrap_or_default();
            info.nonce = info.nonce.checked_add(entry.offset).ok_or_else(|| {
                BlockExecutionError::msg(
                    "sequence initialization failed: sequence sender nonce overflow",
                )
            })?;
            state.insert(address, Account::from(info).with_touched_mark());
        }
        Ok(state)
    }
}
