//! Arcology's parallel execution worker set.

use alloy_evm::block::{BlockExecutorFactory, ExecutableTxParts};
use alloy_evm::{Database, EvmEnv, EvmFactory};
use alloy_op_evm::OpBlockExecutionCtx;
use rayon::{ThreadPool, prelude::*};
use std::io::Error;

use alloy_evm::revm::{DatabaseRef, context_interface::Transaction, state::EvmState};

use super::{
    ArcologyEvmFactory, key::Key, op::backend::PostExecEvmFactoryAdapter, outcome::JobOutcome,
    output::ExecutionOutput,
};
use crate::planner::{
    Planner,
    resolver::{Checker, ConflictResult},
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
/// Each sequence owns a database. Its transactions run in order and
/// see preceding changes; independent sequences share a persistent worker pool.
#[derive(Clone, Copy, Debug, Default)]
pub struct ArcologyParaBlockExecutor;

impl ArcologyParaBlockExecutor {
    /// Orders the results of a whole block by original tx index, then rewrites each
    /// receipt's cumulative gas as the running total up to and including its tx.
    /// Jobs returning an error don't count toward the total.
    pub fn finish_block<Receipt, HaltReason>(
        mut results: Vec<JobOutcome<Receipt, HaltReason>>,
        set_cumulative: &impl Fn(&mut Receipt, u64),
    ) -> Vec<JobOutcome<Receipt, HaltReason>> {
        // Please note that the results are sorted by the original transaction index.
        // So the execution order is NOT necessarily the same as the original order.
        results.sort_by_key(|result| result.id);
        Self::accumulate(&mut results, set_cumulative);
        results
    }

    /// Writes the running gas total into each executed transaction's receipt, in the given order,
    /// and returns the total.
    pub fn accumulate<Receipt, HaltReason>(
        results: &mut [JobOutcome<Receipt, HaltReason>],
        set_cumulative: &impl Fn(&mut Receipt, u64),
    ) -> u64 {
        let mut cumulative = 0;
        for result in results {
            if let (Ok(executed), Some(receipt)) = (&result.result_and_state, &mut result.receipt) {
                cumulative += executed.result.tx_gas_used();
                set_cumulative(receipt, cumulative);
            }
        }
        cumulative
    }
    /// Executes a block: schedules the ordered transactions, runs the generations in turn
    /// and returns ordered outcomes and each generation's conflict/metadata result.
    pub fn execute_block<'db, F, PDB, Fallback, TxEnv, R>(
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        set_cumulative: &impl Fn(&mut F::Receipt, u64),
        fallback: &'db Fallback,
        planner: &mut Planner<PDB, Key>,
        committer: &mut impl Committer<ID, EvmState>,
        transactions: impl IntoIterator<Item = (TxEnv, R)>,
    ) -> Result<
        (
            Vec<JobOutcome<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
            Vec<ConflictResult<Key>>,
        ),
        Error,
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
        (TxEnv, R): Send + ExecutableTxParts<<F::EvmFactory as EvmFactory>::Tx, F::Transaction>,
    {
        // Convert the incoming transactions into jobs with assigned IDs.
        let jobs = transactions
            .into_iter()
            .enumerate()
            .map(|(index, tx)| Job::new(ID::new(index as u64), tx));

        let mut results = Vec::new();
        let mut resolutions = Vec::new();
        for generation in planner.plan(jobs).map_err(Error::other)? {
            let (generation_results, conflicts) = Self::run_generation(
                factory,
                config,
                pool,
                fallback,
                generation,
                &mut planner.checker,
                committer,
            )?;
            resolutions.push(planner.resolve(conflicts).map_err(Error::other)?);
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
        generation: Generation<Tx>,
        checker: &mut impl Checker<Key>,
        committer: &mut impl Committer<ID, EvmState>,
    ) -> Result<
        (
            Vec<JobOutcome<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
            ConflictResult<Key>,
        ),
        Error,
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
        Tx: Send + ExecutableTxParts<<F::EvmFactory as EvmFactory>::Tx, F::Transaction>,
    {
        // Indexed collection preserves plan order across parallel workers.
        let results: Result<Vec<_>, Error> = pool.install(|| {
            generation
                .sequences
                .into_par_iter()
                .map(|sequence| Self::run_sequence(factory, config, sequence, fallback))
                .collect()
        });
        let results: Vec<_> = results?.into_iter().flatten().collect();

        for outcome in &results {
            let Ok(executed) = &outcome.result_and_state else {
                continue;
            };

            let id = outcome.id;

            // Buffer the executed state sets by their transaction IDs
            committer.buffer(id, executed.state.clone());

            // Merge reads and writes for the checker
            let records = outcome.merge_records(id);

            checker.import(
                records
                    .into_iter()
                    .map(|(key, data)| ExecutionOutput {
                        id: outcome.id,
                        key,
                        gas_used: executed.result.tx_gas_used(),
                        accumulated_gas: 0,
                        data,
                    })
                    .collect(),
            );
        }

        // Check for conflicts among the imported records before staging committable outcomes.
        let conflicts = checker.check();
        committer.stage(
            results
                .iter()
                .filter(|outcome| conflicts.committable.contains(&outcome.id))
                .map(|outcome| outcome.id)
                .collect(),
        );
        committer.flush();
        Ok((results, conflicts))
    }

    /// Runs a sequence over the shared read-only fallback.
    fn run_sequence<'db, F, Fallback, Tx>(
        factory: &F,
        config: &ParaBlockConfig<F>,
        sequence: JobSequence<Tx>,
        fallback: &'db Fallback,
    ) -> Result<Vec<JobOutcome<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>, Error>
    where
        F: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>
            + Send
            + Sync,
        for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
        <F::EvmFactory as EvmFactory>::Spec: Clone + Send + Sync,
        <F::EvmFactory as EvmFactory>::BlockEnv: Clone + Send + Sync,
        Fallback: DatabaseRef + std::fmt::Debug,
        F::Receipt: Clone + Send,
        Tx: Send + ExecutableTxParts<<F::EvmFactory as EvmFactory>::Tx, F::Transaction>,
    {
        sequence.run(factory, fallback, config.env.clone(), config.ctx.clone())
    }
}

#[cfg(test)]
mod generation_tests;
