//! Arcology's parallel execution worker set.

use alloy_evm::block::{BlockExecutorFactory, ExecutableTxParts};
use alloy_evm::{Database, EvmFactory};
use alloy_op_evm::OpBlockExecutionCtx;
use rayon::ThreadPool;
use std::io::Error;

use alloy_evm::revm::{context_interface::Transaction, state::EvmState};

use super::{
    ArcologyEvmFactory, ParaBlockConfig,
    key::Key,
    op::backend::PostExecEvmFactoryAdapter,
    workload::{ID, Job, JobResult, ReceiptStatus},
};
use crate::planner::Planner;
use crate::store::cache::PersistentStore;

/// Stateless processor that prepares block workloads and finalizes transaction outcomes.
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
            if let Some(gas) = result.gas_used() {
                if let Some(receipt) = &mut result.receipt {
                    cumulative += gas;
                    set_cumulative(receipt, cumulative);
                }
            }
        }
        cumulative
    }

    /// Prepares jobs, validates and schedules them against the block gas limit, and executes them
    /// over `cache`; returns the results and the changes, committed once consensus accepts them.
    pub fn execute_block<F, PDB, TxEnv, R>(
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        set_cumulative: &impl Fn(&mut F::Receipt, u64),
        cache: &PersistentStore<'_>,
        planner: &mut Planner<PDB, Key>,
        transactions: impl IntoIterator<Item = (TxEnv, R)>,
    ) -> Result<
        (
            Vec<JobResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
            EvmState,
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
        F::Receipt: Clone + Send + ReceiptStatus,
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

        planner.block_gas_limit = config.env.block_env.gas_limit;
        let block = planner.plan(jobs).map_err(Error::other)?;
        block.run(factory, config, pool, set_cumulative, cache, planner)
    }
}

#[cfg(test)]
#[path = "tests/executor_generation.rs"]
mod generation_tests;

#[cfg(test)]
#[path = "tests/fee_settlement.rs"]
mod fee_settlement_tests;
