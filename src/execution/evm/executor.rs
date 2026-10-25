//! Arcology's parallel execution worker set.

use alloy_evm::block::{
    BlockExecutionError, BlockExecutor, BlockExecutorFactory, ExecutableTx, StateDB, TxResult,
};
use alloy_evm::{Evm, EvmEnv, EvmFactory};
use alloy_op_evm::OpBlockExecutionCtx;
use alloy_primitives::Address;
use rayon::{ThreadPool, ThreadPoolBuilder, prelude::*};

use alloy_evm::revm::{
    context_interface::Transaction,
    state::{Account, EvmState},
};

use super::{
    ReadRecorder,
    outcome::{ScheduledTransactionResult, TransactionOutcome},
};
use crate::scheduler::{Generation, ID, Job, JobSequence, Scheduler};

/// Settings shared by every worker of a block.
pub struct ParaBlockConfig<F: BlockExecutorFactory> {
    pub workers: usize,
    pub env: EvmEnv<<F::EvmFactory as EvmFactory>::Spec, <F::EvmFactory as EvmFactory>::BlockEnv>,
    pub ctx: OpBlockExecutionCtx,
}

/// Runs the transactions of a generation on parallel workers.
///
/// Each sequence owns an executor and database. Its transactions run in order and
/// see preceding changes; independent sequences share a persistent worker pool.
pub struct ArcologyParaBlockExecutor<F: BlockExecutorFactory> {
    factory: F,
    config: ParaBlockConfig<F>,
    pool: ThreadPool,
    set_cumulative: Box<dyn Fn(&mut F::Receipt, u64) + Send + Sync>,
}

impl<F: BlockExecutorFactory> ArcologyParaBlockExecutor<F> {
    /// `set_cumulative` writes a cumulative gas value into a receipt.
    pub fn new(
        factory: F,
        config: ParaBlockConfig<F>,
        set_cumulative: impl Fn(&mut F::Receipt, u64) + Send + Sync + 'static,
    ) -> Self {
        // Keep the workers alive across generations and block executions.
        let pool = ThreadPoolBuilder::new()
            .num_threads(config.workers.max(1))
            .build()
            .expect("failed to create EVM worker pool");
        Self {
            factory,
            config,
            pool,
            set_cumulative: Box::new(set_cumulative),
        }
    }

    /// Orders the results of a whole block by original tx index, then rewrites each
    /// receipt's cumulative gas as the running total up to and including its tx.
    /// Failed transactions don't count toward the total.
    pub fn finish_block(
        &self,
        mut results: Vec<
            ScheduledTransactionResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>,
        >,
    ) -> Vec<ScheduledTransactionResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>
    {
        // Please note that the results are sorted by the original transaction index.
        // So the execution order is NOT necessarily the same as the original order.
        results.sort_by_key(|result| result.id);
        self.apply_cumulative_gas(&mut results);
        results
    }

    /// Writes the running gas total into each successful receipt, in the given order,
    /// and returns the total.
    pub fn apply_cumulative_gas(
        &self,
        results: &mut [ScheduledTransactionResult<
            F::Receipt,
            <F::EvmFactory as EvmFactory>::HaltReason,
        >],
    ) -> u64 {
        let mut cumulative = 0;
        for result in results {
            if let Ok(execution) = &mut result.outcome {
                cumulative += execution.result_and_state.result.tx_gas_used();
                (self.set_cumulative)(&mut execution.receipt, cumulative);
            }
        }
        cumulative
    }
}

impl<F> ArcologyParaBlockExecutor<F>
where
    F: BlockExecutorFactory + Send + Sync,
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
    ) -> Vec<ScheduledTransactionResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>
    where
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

    /// Executes independent sequences in parallel, keeping each sequence intact.
    /// Returns after all sequences finish; block ordering and gas are finalized separately.
    fn para_execute_generation<DB, Tx>(
        &self,
        make_db: &(impl Fn() -> DB + Sync),
        generation: Generation<Tx>,
    ) -> Vec<ScheduledTransactionResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>
    where
        DB: StateDB,
        F::Receipt: Clone + Send,
        Tx: Send + for<'a> ExecutableTx<F::Executor<'a, DB, ReadRecorder>>,
    {
        // Avoid dispatch overhead when there is no parallel work.
        if generation.sequences.len() <= 1 || self.pool.current_num_threads() == 1 {
            return generation
                .sequences
                .into_iter()
                .flat_map(|sequence| self.execute_sequence(make_db, sequence))
                .collect();
        }

        // Workers take whole sequences as they become available. Indexed collection
        // preserves plan order without coupling execution to fixed transaction chunks.
        let results: Vec<_> = self.pool.install(|| {
            generation
                .sequences
                .into_par_iter()
                .map(|sequence| self.execute_sequence(make_db, sequence))
                .collect()
        });
        results.into_iter().flatten().collect()
    }

    /// Reuses one executor so committed transaction changes reach the next job.
    fn execute_sequence<DB, Tx>(
        &self,
        make_db: &(impl Fn() -> DB + Sync),
        sequence: JobSequence<Tx>,
    ) -> Vec<ScheduledTransactionResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>
    where
        DB: StateDB,
        F::Receipt: Clone + Send,
        Tx: Send + for<'a> ExecutableTx<F::Executor<'a, DB, ReadRecorder>>,
    {
        let Some(first) = sequence.jobs.first() else {
            return Vec::new();
        };
        let mut db = make_db();

        // Account for each sender's earlier jobs in other sequences, once at startup.
        let setup = sequence.nonce_offsets.into_iter().try_for_each(|entry| {
            if entry.offset == 0 {
                return Ok(());
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
            db.commit(EvmState::from_iter([(
                address,
                Account::from(info).with_touched_mark(),
            )]));
            Ok::<_, BlockExecutionError>(())
        });
        if let Err(error) = setup {
            return sequence
                .jobs
                .into_iter()
                .map(|job| ScheduledTransactionResult {
                    id: job.id,
                    outcome: Err(BlockExecutionError::msg(format!(
                        "sequence initialization failed: {error}"
                    ))),
                })
                .collect();
        }

        let evm = self.factory.evm_factory().create_evm_with_inspector(
            db,
            self.config.env.clone(),
            ReadRecorder::new(first.id.global),
        );
        let mut executor = self.factory.create_executor(evm, self.config.ctx.clone());
        let mut results = Vec::with_capacity(sequence.jobs.len());
        for job in sequence.jobs {
            let id = job.id;
            // Reads and their IDs belong to this transaction, even after a failed job.
            executor.evm_mut().components_mut().1.reset(id.global);
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
                let inspector_reads = executor.evm_mut().components_mut().1.drain();
                Ok(TransactionOutcome {
                    result_and_state: result_and_state
                        .expect("a successful execution has a result"),
                    receipt,
                    inspector_reads,
                })
            })();
            results.push(ScheduledTransactionResult { id, outcome });
        }
        results
    }
}
