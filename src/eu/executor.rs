//! Arcology's parallel execution worker set.

use alloy_evm::block::{BlockExecutionError, BlockExecutorFactory, ExecutableTx};
use alloy_evm::{Database, EvmEnv, EvmFactory};
use alloy_op_evm::OpBlockExecutionCtx;
use rayon::{ThreadPool, prelude::*};

use alloy_evm::revm::{DatabaseRef, context_interface::Transaction};

use super::{
    ArcologyEvmFactory, ReadRecorder, StatDbCommitter, StateDb, key::Key,
    op::backend::PostExecEvmFactoryAdapter, outcome::JobOutcome, output::ExecutionOutput,
};
use crate::planner::{
    Planner,
    resolver::{ConflictChecker, ConflictResult},
    scheduler::{Generation, ID, Job, JobSequence},
};
use crate::store::traits::Committer;

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
        mut results: Vec<JobOutcome<Receipt, HaltReason>>,
        set_cumulative: &impl Fn(&mut Receipt, u64),
    ) -> Vec<JobOutcome<Receipt, HaltReason>> {
        // Please note that the results are sorted by the original transaction index.
        // So the execution order is NOT necessarily the same as the original order.
        results.sort_by_key(|result| result.id);
        Self::accumulate_gas(&mut results, set_cumulative);
        results
    }

    /// Writes the running gas total into each successful receipt, in the given order,
    /// and returns the total.
    pub fn accumulate_gas<Receipt, HaltReason>(
        results: &mut [JobOutcome<Receipt, HaltReason>],
        set_cumulative: &impl Fn(&mut Receipt, u64),
    ) -> u64 {
        let mut cumulative = 0;
        for result in results {
            cumulative += result.result_and_state.result.tx_gas_used();
            set_cumulative(&mut result.receipt, cumulative);
        }
        cumulative
    }
    /// Executes a block: schedules the ordered transactions, runs the generations in turn
    /// and returns ordered outcomes and each generation's conflict/history result.
    pub fn execute_block<'db, F, PDB, Fallback, TxEnv, R>(
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        set_cumulative: &impl Fn(&mut F::Receipt, u64),
        fallback: &'db Fallback,
        planner: &mut Planner<PDB, Key>,
        committer: &mut StatDbCommitter,
        transactions: impl IntoIterator<Item = (TxEnv, R)>,
    ) -> Result<
        (
            Vec<JobOutcome<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
            Vec<ConflictResult<Key>>,
        ),
        BlockExecutionError,
    >
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>
            + Send
            + Sync,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        <F::EvmFactory as EvmFactory>::Spec: Clone + Send + Sync,
        <F::EvmFactory as EvmFactory>::BlockEnv: Clone + Send + Sync,
        PDB: Database,
        Fallback: DatabaseRef + Sync + std::fmt::Debug,
        F::Receipt: Clone + Send,
        TxEnv: Transaction,
        (TxEnv, R):
            Send + for<'a> ExecutableTx<F::Executor<'a, StateDb<&'db Fallback>, ReadRecorder>>,
    {
        // Convert the incoming transactions into jobs with assigned IDs.
        let jobs = transactions
            .into_iter()
            .enumerate()
            .map(|(index, tx)| Job::new(ID::new(index as u64), tx));

        let mut results = Vec::new();
        let mut resolutions = Vec::new();
        for generation in planner.plan(jobs)? {
            let (generation_results, conflicts) = Self::run_generation(
                factory,
                config,
                pool,
                fallback,
                committer,
                generation,
                &mut planner.checker,
            )?;
            resolutions.push(planner.resolve(conflicts)?);
            results.extend(generation_results);
        }

        // Finish the block by restoring block order and applying cumulative gas.
        Ok((Self::finish_block(results, set_cumulative), resolutions))
    }

    /// Executes independent sequences in parallel, keeping each sequence intact.
    /// Checks conflicts and merges accepted results into the committer's working state.
    fn run_generation<'db, F, Fallback, Tx>(
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        fallback: &'db Fallback,
        committer: &mut StatDbCommitter,
        generation: Generation<Tx>,
        checker: &mut ConflictChecker<Key>,
    ) -> Result<
        (
            Vec<JobOutcome<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
            ConflictResult<Key>,
        ),
        BlockExecutionError,
    >
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>
            + Send
            + Sync,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        <F::EvmFactory as EvmFactory>::Spec: Clone + Send + Sync,
        <F::EvmFactory as EvmFactory>::BlockEnv: Clone + Send + Sync,
        Fallback: DatabaseRef + Sync + std::fmt::Debug,
        F::Receipt: Clone + Send,
        Tx: Send + for<'a> ExecutableTx<F::Executor<'a, StateDb<&'db Fallback>, ReadRecorder>>,
    {
        // Avoid dispatch overhead when there is no parallel work.
        let results = if generation.sequences.len() <= 1 || pool.current_num_threads() == 1 {
            let mut results = Vec::new();
            for sequence in generation.sequences {
                results.extend(Self::run_sequence(factory, config, sequence, fallback)?);
            }
            results
        } else {
            // Indexed collection preserves plan order across parallel workers.
            let results: Result<Vec<_>, BlockExecutionError> = pool.install(|| {
                generation
                    .sequences
                    .into_par_iter()
                    .map(|sequence| Self::run_sequence(factory, config, sequence, fallback))
                    .collect()
            });
            results?.into_iter().flatten().collect()
        };

        let mut empty_ids = Vec::new();
        for outcome in &results {
            let id = outcome.id.global;
            committer.buffer(id, outcome.result_and_state.state.clone());
            let gas_used = outcome.result_and_state.result.tx_gas_used();
            let records = outcome.merge_records(id);
            if records.is_empty() {
                empty_ids.push(id);
            }
            checker.import(
                records
                    .into_iter()
                    .map(|(key, data)| ExecutionOutput {
                        id: outcome.id,
                        key,
                        gas_used,
                        data,
                    })
                    .collect(),
            );
        }
        let mut conflicts = checker.check_conflicts();
        conflicts.committable.extend(empty_ids);
        committer.stage(conflicts.committable.iter().copied().collect());
        committer.flush();
        Ok((results, conflicts))
    }

    /// Runs a sequence over the shared read-only fallback.
    fn run_sequence<'db, F, Fallback, Tx>(
        factory: &F,
        config: &ParaBlockConfig<F>,
        sequence: JobSequence<Tx>,
        fallback: &'db Fallback,
    ) -> Result<
        Vec<JobOutcome<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
        BlockExecutionError,
    >
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>
            + Send
            + Sync,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        <F::EvmFactory as EvmFactory>::Spec: Clone + Send + Sync,
        <F::EvmFactory as EvmFactory>::BlockEnv: Clone + Send + Sync,
        Fallback: DatabaseRef + std::fmt::Debug,
        F::Receipt: Clone + Send,
        Tx: Send + for<'a> ExecutableTx<F::Executor<'a, StateDb<&'db Fallback>, ReadRecorder>>,
    {
        sequence.run(factory, fallback, config.env.clone(), config.ctx.clone())
    }
}

#[cfg(test)]
mod generation_tests;
