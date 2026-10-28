//! Arcology's parallel execution worker set.

use alloy_evm::block::{
    BlockExecutionError, BlockExecutor, BlockExecutorFactory, ExecutableTx, StateDB, TxResult,
};
use alloy_evm::{EvmEnv, EvmFactory};
use alloy_op_evm::OpBlockExecutionCtx;
use alloy_primitives::Address;
use rayon::{ThreadPool, prelude::*};

use alloy_evm::revm::{
    context_interface::Transaction,
    state::{Account, EvmState},
};

use super::{
    ArcologyEvmFactory, EvmStateAccumulator, ReadRecorder,
    key::Key,
    op::backend::PostExecEvmFactoryAdapter,
    outcome::{JobResult, TransactionOutcome},
};
use crate::{
    execution::output::ExecutionOutput,
    resolver::ConflictChecker,
    scheduler::{Generation, ID, Job, JobSequence, Scheduler},
    store::traits::Committer,
};

/// Settings shared by every worker of a block.
pub struct ParaBlockConfig<F: BlockExecutorFactory> {
    pub env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
    pub ctx: OpBlockExecutionCtx,
}

/// Runs the transactions of a generation on parallel workers.
///
/// Each sequence owns an executor and database. Its transactions run in order and
/// see preceding changes; independent sequences share a persistent worker pool.
#[derive(Clone, Copy, Debug, Default)]
pub struct ArcologyParaBlockExecutor;

impl ArcologyParaBlockExecutor {
    /// Orders the results of a whole block by original tx index, then rewrites each
    /// receipt's cumulative gas as the running total up to and including its tx.
    /// Failed transactions don't count toward the total.
    pub fn finish_block<Receipt, HaltReason>(
        mut results: Vec<JobResult<Receipt, HaltReason>>,
        set_cumulative: &impl Fn(&mut Receipt, u64),
    ) -> Vec<JobResult<Receipt, HaltReason>> {
        // Please note that the results are sorted by the original transaction index.
        // So the execution order is NOT necessarily the same as the original order.
        results.sort_by_key(|result| result.id);
        Self::accumulate_gas(&mut results, set_cumulative);
        results
    }

    /// Writes the running gas total into each successful receipt, in the given order,
    /// and returns the total.
    pub fn accumulate_gas<Receipt, HaltReason>(
        results: &mut [JobResult<Receipt, HaltReason>],
        set_cumulative: &impl Fn(&mut Receipt, u64),
    ) -> u64 {
        let mut cumulative = 0;
        for result in results {
            if let Ok(execution) = &mut result.outcome {
                cumulative += execution.result_and_state.result.tx_gas_used();
                set_cumulative(&mut execution.receipt, cumulative);
            }
        }
        cumulative
    }
    /// Applies every sender nonce offset with one database commit.
    fn offset_nonces<DB: StateDB, Tx>(
        db: &mut DB,
        sequence: &JobSequence<Tx>,
    ) -> Result<(), BlockExecutionError> {
        let mut state = EvmState::default();
        for entry in &sequence.nonce_offsets {
            if entry.offset == 0 {
                continue;
            }

            let address = Address::from(entry.sender);
            let mut info = db
                .basic(address)
                .map_err(BlockExecutionError::other)?
                .unwrap_or_default();
            info.nonce = info
                .nonce
                .checked_add(entry.offset)
                .ok_or_else(|| BlockExecutionError::msg("sequence sender nonce overflow"))?;
            state.insert(address, Account::from(info).with_touched_mark());
        }

        if !state.is_empty() {
            db.commit(state);
        }
        Ok(())
    }

    /// Executes a block: schedules the ordered transactions, runs the generations in turn
    /// and returns the results in block order with cumulative gas applied.
    pub fn execute_block<F, DB, TxEnv, R>(
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        set_cumulative: &impl Fn(&mut F::Receipt, u64),
        make_db: &(impl Fn(&EvmState) -> DB + Sync),
        scheduler: &impl Scheduler<(TxEnv, R)>,
        resolver: &mut ConflictChecker<Key>,
        accumulator: &mut EvmStateAccumulator,
        transactions: impl IntoIterator<Item = (TxEnv, R)>,
    ) -> Vec<JobResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>
            + Send
            + Sync,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        <F::EvmFactory as EvmFactory>::Spec: Clone + Send + Sync,
        <F::EvmFactory as EvmFactory>::BlockEnv: Clone + Send + Sync,
        DB: StateDB,
        F::Receipt: Clone + Send,
        TxEnv: Transaction,
        (TxEnv, R): Send + for<'a> ExecutableTx<F::Executor<'a, DB, ReadRecorder>>,
    {
        // Convert the incoming transactions into jobs with assigned IDs.
        let jobs = transactions
            .into_iter()
            .enumerate()
            .map(|(index, tx)| Job::new(ID::new(index as u64), tx));

        let mut results = Vec::new();
        for generation in scheduler.schedule(jobs) {
            let mut generation_results = Self::para_run_generation(
                factory,
                config,
                pool,
                make_db,
                accumulator.state(),
                generation,
            );
            Self::resolve_generation(&mut generation_results, resolver, accumulator);
            results.extend(generation_results);
        }

        // Finish the block by restoring block order and applying cumulative gas.
        Self::finish_block(results, set_cumulative)
    }

    /// Resolves one generation and merges only accepted transaction states.
    fn resolve_generation<Receipt, HaltReason>(
        results: &[JobResult<Receipt, HaltReason>],
        resolver: &mut ConflictChecker<Key>,
        accumulator: &mut EvmStateAccumulator,
    ) {
        for result in results {
            let Ok(outcome) = &result.outcome else {
                continue;
            };
            let id = result.id.global;
            let gas_used = outcome.result_and_state.result.tx_gas_used();
            resolver.import(
                outcome
                    .merge_records(id)
                    .into_iter()
                    .map(|(key, data)| ExecutionOutput {
                        id,
                        key,
                        gas_used,
                        data,
                    })
                    .collect(),
            );
            accumulator.buffer(id, outcome.result_and_state.state.clone());
        }

        let conflicts = resolver.check_conflicts();
        accumulator.stage(conflicts.committable.into_iter().collect());
        accumulator.flush();
    }

    /// Executes independent sequences in parallel, keeping each sequence intact.
    /// Returns after all sequences finish; block ordering and gas are finalized separately.
    fn para_run_generation<F, DB, Tx>(
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        make_db: &(impl Fn(&EvmState) -> DB + Sync),
        state: &EvmState,
        generation: Generation<Tx>,
    ) -> Vec<JobResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>
            + Send
            + Sync,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        <F::EvmFactory as EvmFactory>::Spec: Clone + Send + Sync,
        <F::EvmFactory as EvmFactory>::BlockEnv: Clone + Send + Sync,
        DB: StateDB,
        F::Receipt: Clone + Send,
        Tx: Send + for<'a> ExecutableTx<F::Executor<'a, DB, ReadRecorder>>,
    {
        // Avoid dispatch overhead when there is no parallel work.
        if generation.sequences.len() <= 1 || pool.current_num_threads() == 1 {
            return generation
                .sequences
                .into_iter()
                .flat_map(|sequence| Self::run_sequence(factory, config, make_db, state, sequence))
                .collect();
        }

        // Workers take whole sequences as they become available. Indexed collection
        // preserves plan order without coupling execution to fixed transaction chunks.
        let results: Vec<_> = pool.install(|| {
            generation
                .sequences
                .into_par_iter()
                .map(|sequence| Self::run_sequence(factory, config, make_db, state, sequence))
                .collect()
        });
        results.into_iter().flatten().collect()
    }

    /// Reuses one executor so committed transaction changes reach the next job.
    fn run_sequence<F, DB, Tx>(
        factory: &F,
        config: &ParaBlockConfig<F>,
        make_db: &(impl Fn(&EvmState) -> DB + Sync),
        state: &EvmState,
        sequence: JobSequence<Tx>,
    ) -> Vec<JobResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>
            + Send
            + Sync,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        <F::EvmFactory as EvmFactory>::Spec: Clone + Send + Sync,
        <F::EvmFactory as EvmFactory>::BlockEnv: Clone + Send + Sync,
        DB: StateDB,
        F::Receipt: Clone + Send,
        Tx: Send + for<'a> ExecutableTx<F::Executor<'a, DB, ReadRecorder>>,
    {
        let Some(first) = sequence.jobs.first() else {
            return Vec::new();
        };
        let mut db = make_db(state);

        if let Err(error) = Self::offset_nonces(&mut db, &sequence) {
            return sequence
                .jobs
                .into_iter()
                .map(|job| JobResult {
                    id: job.id,
                    outcome: Err(BlockExecutionError::msg(format!(
                        "sequence initialization failed: {error}"
                    ))),
                })
                .collect();
        }

        let evm = factory.evm_factory().create_evm_with_inspector(
            db,
            config.env.clone(),
            ReadRecorder::new(first.id.global),
        );

        let mut executor = factory.create_executor(evm, config.ctx.clone());
        let mut results = Vec::with_capacity(sequence.jobs.len());
        for job in sequence.jobs {
            let id = job.id;
            // Reads and their IDs belong to this transaction, even after a failed job.
            executor.evm_mut().reset_reads(id.global);
            let outcome = (|| {
                let mut result_and_state = None;
                // This commits into the sequence's private DB before the next job starts.
                executor.execute_transaction_with_result_closure(job.tx, |result| {
                    result_and_state = Some(result.result().clone());
                })?;
                let receipt = executor
                    .receipts()
                    .last()
                    .cloned()
                    .expect("a committed transaction has a receipt");
                let merged_reads = executor.evm_mut().drain_reads();
                Ok(TransactionOutcome {
                    result_and_state: result_and_state
                        .expect("a successful execution has a result"),
                    receipt,
                    reads: merged_reads,
                })
            })();
            results.push(JobResult { id, outcome });
        }
        results
    }
}
