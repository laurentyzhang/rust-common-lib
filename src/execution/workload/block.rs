use super::{Generation, JobResult, ReceiptStatus, TransactionGasLimit};
use crate::execution::evm::state::{StateOverlay, sharded, store::OverlayWrite};
use crate::store::cache;
use crate::execution::{ArcologyEvmFactory, ArcologyParaBlockExecutor, StateDb, key::Key};
use crate::planner::{Planner, resolver::CommitDecision};
use alloy_evm::{
    Database, EvmEnv, EvmFactory,
    block::{BlockExecutorFactory, ExecutableTxParts},
    revm::{DatabaseRef, state::EvmState},
};
use alloy_op_evm::{OpBlockExecutionCtx, post_exec::PostExecEvmFactoryAdapter};
use rayon::ThreadPool;
use std::io::Error;

/// Scheduled generations belonging to a block.
pub struct Block<Tx> {
    pub generations: Vec<Generation<Tx>>,
}

impl<Tx> Block<Tx> {
    /// Executes planned generations, carrying unused gas forward after each generation.
    pub fn run<F, PDB, Fallback>(
        self,
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        set_cumulative: &impl Fn(&mut F::Receipt, u64),
        cache: &mut sharded::Sharded,
        fallback: &Fallback,
        planner: &mut Planner<PDB, Key>,
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
        Fallback: DatabaseRef + Sync + std::fmt::Debug,
        F::Receipt: Clone + Send + ReceiptStatus,
        Tx: Send
            + TransactionGasLimit
            + ExecutableTxParts<<F::EvmFactory as EvmFactory>::Tx, F::Transaction>,
    {
        // The cache, kept across blocks in front of the store, holds Sets expanded.
        let cache_db = cache::CacheDb::new(&*cache, fallback);
        // Shared by all sequences; each generation adds its sequences' balance and nonce deltas.
        let mut state: StateDb<_, sharded::Sharded> = StateDb::with_fallback(
            StateOverlay::new(sharded::Sharded::new(), StateOverlay::add_balance_nonce_deltas),
            Some(&cache_db),
        );
        let mut results = Vec::new();
        let mut remaining_gas = config.env.block_env.gas_limit;
        for generation in self.generations {
            planner.checker.set_gas_limit(remaining_gas);
            let (raws, next) = generation.run(factory, config, pool, &mut planner.checker, state)?;
            state = next;
            remaining_gas -= raws.iter().filter_map(|raw| raw.gas_used()).sum::<u64>();
            results.extend(raws);
        }

        // The block's results, without accounts only read, in the execution format.
        let mut merged = state.into_inner();
        merged.retain(|_, account| account.is_touched());
        let mut block_db = StateDb::with_fallback(
            StateOverlay::new(merged, StateOverlay::copy_balance_nonce),
            Some(&cache_db),
        );

        let metadata = planner
            .build_conflict_meta_updates(
                results
                    .iter()
                    .filter(|raw| raw.verdict.decision != CommitDecision::Pending)
                    .map(|raw| &raw.verdict),
                block_db.inner(),
            )
            .map_err(Error::other)?;
        block_db
            .commit_with_original_slots(metadata)
            .map_err(|error| Error::other(format!("failed to commit planner metadata. {error}")))?;

        let changes = block_db.into_inner();
        // The cache keeps the block's results in the execution format, ready for the next block;
        // the store's committer compacts them.
        drop(cache_db);
        cache.write_accounts(changes.clone());
        Ok((
            ArcologyParaBlockExecutor::finish_block(results, set_cumulative),
            changes,
        ))
    }
}

/// Settings shared by every worker of a block.
pub struct ParaBlockConfig<F: BlockExecutorFactory> {
    pub env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
    pub ctx: OpBlockExecutionCtx,
}
