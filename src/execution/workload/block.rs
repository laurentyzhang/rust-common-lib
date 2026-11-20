use super::{Generation, JobResult, ReceiptStatus};
use crate::execution::evm::state::StateOverlay;
use crate::execution::{ArcologyEvmFactory, ArcologyParaBlockExecutor, StateDb, key::Key};
use crate::planner::{Planner, resolver::CommitDecision};
use alloy_evm::{
    Database, EvmEnv, EvmFactory,
    block::{BlockExecutorFactory, ExecutableTxParts},
    revm::{DatabaseCommit, DatabaseRef, state::EvmState},
};
use alloy_op_evm::{OpBlockExecutionCtx, post_exec::PostExecEvmFactoryAdapter};
use rayon::ThreadPool;
use std::io::Error;

/// Scheduled generations belonging to a block.
pub struct Block<Tx> {
    pub generations: Vec<Generation<Tx>>,
}

impl<Tx> Block<Tx> {
    /// Executes planned generations and returns finalized outcomes and committed block state.
    pub fn run<F, PDB, Fallback>(
        self,
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        set_cumulative: &impl Fn(&mut F::Receipt, u64),
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
        Tx: Send + ExecutableTxParts<<F::EvmFactory as EvmFactory>::Tx, F::Transaction>,
    {
        let mut block_db = StateDb::with_fallback(
            StateOverlay::new(EvmState::default(), StateOverlay::copy_balance_nonce),
            Some(fallback),
        );
        let mut results = Vec::new();
        let mut remaining_gas = config.env.block_env.gas_limit;
        for generation in self.generations {
            planner.checker.set_gas_limit(remaining_gas);
            let (raws, committed) =
                generation.run(factory, config, pool, &mut planner.checker, &block_db)?;
            remaining_gas -= raws.iter().filter_map(|raw| raw.gas_used()).sum::<u64>();
            // Publish accepted changes before executing the next generation.
            block_db.commit(committed);
            if let Some(error) = block_db.commit_error() {
                return Err(Error::other(format!(
                    "failed to commit generation state. {error}"
                )));
            }
            results.extend(raws);
        }

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

        Ok((
            ArcologyParaBlockExecutor::finish_block(results, set_cumulative),
            block_db.into_inner(),
        ))
    }
}

/// Settings shared by every worker of a block.
pub struct ParaBlockConfig<F: BlockExecutorFactory> {
    pub env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
    pub ctx: OpBlockExecutionCtx,
}
