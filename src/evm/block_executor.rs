//! Arcology's parallel execution worker set.

use alloy_evm::block::{
    BlockExecutionError, BlockExecutor, BlockExecutorFactory, ExecutableTx, GasOutput, StateDB,
};
use alloy_evm::revm::inspector::NoOpInspector;
use alloy_evm::{EvmEnv, EvmFactory};
use alloy_op_evm::OpBlockExecutionCtx;

use alloy_evm::revm::context_interface::Transaction;

use crate::scheduler::{Generation, Job, Scheduler};

/// Settings shared by every worker of a block.
pub struct ParaBlockConfig<F: BlockExecutorFactory> {
    pub workers: usize,
    pub env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
    pub ctx: OpBlockExecutionCtx,
}

/// A result tagged with the job's original index in the block.
pub type IndexedResult<R> = (u64, Result<(GasOutput, R), BlockExecutionError>);

/// Runs the transactions of a generation on parallel workers.
///
/// Every transaction gets a new executor whose EVM has its own database from the
/// `make_db` passed to each call and a copy of the shared `config.env`.
pub struct ArcologyParaBlockExecutor<F: BlockExecutorFactory> {
    factory: F,
    config: ParaBlockConfig<F>,
    set_cumulative: Box<dyn Fn(&mut F::Receipt, u64) + Send + Sync>,
}

impl<F: BlockExecutorFactory> ArcologyParaBlockExecutor<F> {
    /// `set_cumulative` writes a cumulative gas value into a receipt.
    pub(super) fn new(
        factory: F,
        config: ParaBlockConfig<F>,
        set_cumulative: impl Fn(&mut F::Receipt, u64) + Send + Sync + 'static,
    ) -> Self {
        Self {
            factory,
            config,
            set_cumulative: Box::new(set_cumulative),
        }
    }

    /// Orders the results of a whole block by original tx index, then rewrites each
    /// receipt's cumulative gas as the running total up to and including its tx.
    /// Failed transactions don't count toward the total.
    pub fn finish_block(
        &self,
        mut results: Vec<IndexedResult<F::Receipt>>,
    ) -> Vec<IndexedResult<F::Receipt>> {
        // Please note that the results are sorted by the original transaction index.
        // So the execution order is NOT necessarily the same as the original order.
        results.sort_by_key(|(index, _)| *index);
        self.apply_cumulative_gas(&mut results);
        results
    }

    /// Writes the running gas total into each successful receipt, in the given order,
    /// and returns the total.
    pub fn apply_cumulative_gas(&self, results: &mut [IndexedResult<F::Receipt>]) -> u64 {
        let mut cumulative = 0;
        for (_, result) in results.iter_mut() {
            if let Ok((gas, receipt)) = result {
                cumulative += gas.tx_gas_used();
                (self.set_cumulative)(receipt, cumulative);
            }
        }
        cumulative
    }
}

impl<F> ArcologyParaBlockExecutor<F>
where
    F: BlockExecutorFactory + Clone + Send + Sync,
    for<'a> F: BlockExecutorFactory<ExecutionCtx<'a> = OpBlockExecutionCtx>,
    <F::EvmFactory as EvmFactory>::Spec: Clone + Send + Sync,
    <F::EvmFactory as EvmFactory>::BlockEnv: Clone + Send + Sync,
{
    /// Executes a block: schedules the ordered transactions, runs the generations in turn
    /// and returns the results in block order with cumulative gas applied.
    pub fn execute_block<DB, TxEnv, R>(
        &self,
        make_db: &(impl Fn() -> DB + Sync),
        scheduler: &impl Scheduler<(TxEnv, R)>,
        transactions: impl IntoIterator<Item = (TxEnv, R)>,
    ) -> Vec<IndexedResult<F::Receipt>>
    where
        DB: StateDB,
        F::Receipt: Clone + Send,
        TxEnv: Transaction,
        (TxEnv, R): Send + for<'a> ExecutableTx<F::Executor<'a, DB, NoOpInspector>>,
    {
        // Convert the incoming transactions into jobs with assigned IDs.
        let jobs = transactions
            .into_iter()
            .enumerate()
            .map(|(index, tx)| Job::new(index as u64, tx));

        // Schedule the jobs using the provided scheduler and execute the resulting generations
        // in sequence with parallelism within each generation.
        let results = scheduler
            .schedule(jobs)
            .into_iter()
            .flat_map(|generation| self.para_execute_generation(make_db, generation))
            .collect();

        // Finish the block by restoring block order and applying cumulative gas.
        self.finish_block(results)
    }

    /// Executes the transactions in one generation parallelly. The jobs are split evenly
    /// across the workers and each worker runs its share in order. Pass the results of all
    /// generations to `finish_block` to restore block order and cumulative gas.
    fn para_execute_generation<DB, Tx>(
        &self,
        make_db: &(impl Fn() -> DB + Sync),
        generation: Generation<Tx>,
    ) -> Vec<IndexedResult<F::Receipt>>
    where
        DB: StateDB,
        F::Receipt: Clone + Send,
        Tx: Send + for<'a> ExecutableTx<F::Executor<'a, DB, NoOpInspector>>,
    {
        let all: Vec<(u64, Tx)> = generation
            .sequences
            .into_iter()
            .flat_map(|seq| seq.jobs)
            .map(|job| (job.id, job.tx))
            .collect();
        let chunk_len = all.len().div_ceil(self.config.workers.max(1)).max(1);
        let mut all = all.into_iter();
        let mut chunks: Vec<Vec<(u64, Tx)>> = Vec::new();
        loop {
            let chunk: Vec<(u64, Tx)> = all.by_ref().take(chunk_len).collect();
            if chunk.is_empty() {
                break;
            }
            chunks.push(chunk);
        }
        let factories: Vec<F> = chunks.iter().map(|_| self.factory.clone()).collect();

        let (env, ctx) = (&self.config.env, &self.config.ctx);
        std::thread::scope(|scope| {
            let handles: Vec<_> = factories
                .iter()
                .zip(chunks)
                .map(|(factory, chunk)| {
                    scope.spawn(move || {
                        chunk
                            .into_iter()
                            .map(|(index, tx)| {
                                let result = (|| {
                                    let evm =
                                        factory.evm_factory().create_evm(make_db(), env.clone());
                                    let mut executor = factory.create_executor(evm, ctx.clone());
                                    let gas = executor.execute_transaction(tx)?;
                                    // The executor is new, so its only receipt belongs to this tx.
                                    let receipt = executor
                                        .receipts()
                                        .last()
                                        .cloned()
                                        .expect("a committed transaction has a receipt");
                                    Ok((gas, receipt))
                                })();
                                (index, result)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().expect("worker panicked"))
                .collect()
        })
    }
}
