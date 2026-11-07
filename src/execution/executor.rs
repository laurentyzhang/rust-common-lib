//! Arcology's parallel execution worker set.

use alloy_evm::block::{BlockExecutorFactory, ExecutableTxParts};
use alloy_evm::{Database, EvmEnv, EvmFactory};
use alloy_op_evm::OpBlockExecutionCtx;
use rayon::ThreadPool;
use std::io::Error;

use alloy_evm::revm::{
    DatabaseCommit, DatabaseRef, context_interface::Transaction, state::EvmState,
};

use super::{
    ArcologyEvmFactory, StateOutcome,
    key::Key,
    op::backend::PostExecEvmFactoryAdapter,
    workload::{ID, Job, JobResult},
};
use crate::planner::{Planner, resolver::ConflictResult};

use crate::execution::evm::StateDb;

/// Settings shared by every worker of a block.
pub struct ParaBlockConfig<F: BlockExecutorFactory> {
    pub env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
    pub ctx: OpBlockExecutionCtx,
}

/// Coordinates block generations and finalizes transaction outcomes.
#[derive(Clone, Copy, Debug, Default)]
pub struct ArcologyParaBlockExecutor;

impl ArcologyParaBlockExecutor {
    /// Orders the results of a whole block by original tx index, then rewrites each
    /// receipt's cumulative gas as the running total up to and including its tx.
    /// Jobs returning an error don't count toward the total.
    pub fn finish_block<Receipt, HaltReason>(
        mut results: Vec<JobResult<Receipt, HaltReason>>,
        set_cumulative: &impl Fn(&mut Receipt, u64),
    ) -> Vec<JobResult<Receipt, HaltReason>> {
        // Please note that the results are sorted by the original transaction index.
        // So the execution order is NOT necessarily the same as the original order.
        results.sort_by_key(|result| result.id);
        Self::accumulate(&mut results, set_cumulative);
        results
    }

    /// Writes the running gas total into each executed transaction's receipt, in the given order,
    /// and returns the total.
    pub fn accumulate<Receipt, HaltReason>(
        results: &mut [JobResult<Receipt, HaltReason>],
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
    /// and returns ordered raw outcomes, committed block state, and generation conflict results.
    pub fn execute_block<'db, F, PDB, Fallback, TxEnv, R>(
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        set_cumulative: &impl Fn(&mut F::Receipt, u64),
        fallback: &'db Fallback,
        planner: &mut Planner<PDB, Key>,
        transactions: impl IntoIterator<Item = (TxEnv, R)>,
    ) -> Result<
        (
            StateOutcome<
                Vec<JobResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
                EvmState,
            >,
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
        let block = config
            .env
            .block_env
            .number
            .try_into()
            .map_err(|_| Error::other("block number exceeds u64"))?;
        let jobs = transactions
            .into_iter()
            .enumerate()
            .map(|(index, tx)| Job::new(ID::new(block, index as u64), tx));

        // Initialize the sequential state database with a fallback to the provided database.
        let mut generation_db = StateDb::with_fallback(EvmState::default(), Some(fallback));

        let mut results = Vec::new();
        let mut resolutions = Vec::new();
        for generation in planner.plan(jobs).map_err(Error::other)? {
            let (generation_result, conflicts) =
                generation.run(factory, config, pool, &mut planner.checker, &generation_db)?;
            // Publish accepted changes before executing the next generation.
            generation_db.commit(generation_result.committed);
            resolutions.push(planner.resolve(conflicts).map_err(Error::other)?);
            results.extend(generation_result.raw);
        }

        // Finish the block by restoring block order and applying cumulative gas.
        Ok((
            StateOutcome {
                raw: Self::finish_block(results, set_cumulative),
                committed: generation_db.into_inner(),
            },
            resolutions,
        ))
    }
}

#[cfg(test)]
#[path = "tests/executor_generation_tests.rs"]
mod generation_tests;
