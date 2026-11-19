use super::{JobResult, JobSequence, ReceiptStatus};
use crate::execution::evm::state::StateOverlay;
use crate::planner::scheduler::sender::{Sender, SenderNonceOffset};
use alloy_evm::{
    EvmFactory,
    block::{BlockExecutorFactory, ExecutableTxParts},
    revm::{DatabaseRef, state::EvmState},
};
use alloy_op_evm::{OpBlockExecutionCtx, post_exec::PostExecEvmFactoryAdapter};
use rayon::{ThreadPool, prelude::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Error,
};

use crate::execution::{ArcologyEvmFactory, EvmStateCommitter, ParaBlockConfig, StateDb, key::Key};
use crate::planner::resolver::Checker;
use crate::store::traits::Committer;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Generation<Tx> {
    pub sequences: Vec<JobSequence<Tx>>,
}

impl<Tx> Default for Generation<Tx> {
    fn default() -> Self {
        Self {
            sequences: Vec::new(),
        }
    }
}

impl<Tx> Generation<Tx> {
    /// Executes sequences in parallel, finalizes job results, and merges sequence states.
    pub fn run<F, Fallback>(
        self,
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        checker: &mut impl Checker<Key>,
        fallback: &Fallback,
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
        Fallback: DatabaseRef + Sync + std::fmt::Debug,
        F::Receipt: Clone + Send + ReceiptStatus,
        Tx: Send + ExecutableTxParts<<F::EvmFactory as EvmFactory>::Tx, F::Transaction>,
    {
        // Use previously committed state as the base for this generation.
        let gen_db = StateDb::with_fallback(
            StateOverlay::new(EvmState::default(), StateOverlay::copy_balance_nonce),
            Some(fallback),
        );

        // Execute sequences in parallel while preserving their planned order.
        let results: Result<Vec<_>, Error> = pool.install(|| {
            self.sequences
                .into_par_iter()
                .map(|sequence| {
                    // Each sequence applies its changes to its own state overlay.
                    sequence.execute(factory, &gen_db, config.env.clone(), config.ctx.clone())
                })
                .collect()
        });
        let sequences = results?;
        // Export each job's accesses and state changes to the checker.
        for (raws, _) in &sequences {
            for raw in raws {
                checker.import(raw.to_records(raw.id));
            }
        }

        // Check gas limits, numeric bounds, and conflicts between sequences.
        let verdicts = checker.check();
        let mut committer = EvmStateCommitter::with_target(gen_db.into_inner());
        let mut raws = Vec::new();
        for (mut results, mut committed) in sequences {
            // Attach checker decisions, then reject jobs dependent on an earlier rejection.
            for raw in &mut results {
                if let Ok(index) = verdicts.binary_search_by_key(&raw.id, |verdict| verdict.id) {
                    raw.verdict = verdicts[index].clone();
                }
            }
            JobSequence::<Tx>::assign_verdicts(results.iter_mut().map(|raw| &mut raw.verdict));
            // Rebuild affected sequences using finalized job results.
            if !JobSequence::<Tx>::is_all_accepted(&results) {
                committed = JobSequence::<Tx>::rebuild_sequence_state(&mut results, fallback)?;
            }
            if let Some(raw) = results.first() {
                committer.buffer(raw.id, committed);
            }
            raws.extend(results);
        }
        // Merge finalized sequence states in their scheduled order.
        Ok((raws, committer.commit(fallback)?))
    }

    /// Sets each job ID's generation to the supplied index and its sequence/job
    /// fields to their zero-based positions, preserving the block and global transaction index.
    pub(crate) fn assign_ids(&mut self, generation: u64) {
        for (sequence, jobs) in self.sequences.iter_mut().enumerate() {
            for (job, scheduled) in jobs.jobs.iter_mut().enumerate() {
                scheduled.id.generation = generation;
                scheduled.id.sequence = sequence as u64;
                scheduled.id.job = job as u64;
            }
        }
    }

    /// Rebuilds nonce offsets for the supplied generation without reordering jobs.
    /// For each sender in a sequence, counts that sender's jobs before its first
    /// job in the generation's nonce and ID ordering. Stores one offset per
    /// sender, including zero offsets, in sender-address order.
    pub(crate) fn new(mut sequences: Vec<JobSequence<Tx>>) -> Self {
        let mut tx_by_sender = BTreeMap::<Sender, Vec<(u64, u64, usize)>>::new();
        for (sequence_index, sequence) in sequences.iter_mut().enumerate() {
            sequence.nonce_offsets.clear();
            for job in &sequence.jobs {
                tx_by_sender.entry(job.sender).or_default().push((
                    job.nonce,
                    job.id.global,
                    sequence_index,
                ));
            }
        }

        for (sender, mut tx) in tx_by_sender {
            tx.sort_unstable_by_key(|(nonce, id, _)| (*nonce, *id));
            let mut with_offset = BTreeSet::new();
            for (offset, (_, _, sequence_index)) in tx.into_iter().enumerate() {
                if with_offset.insert(sequence_index) {
                    sequences[sequence_index]
                        .nonce_offsets
                        .push(SenderNonceOffset {
                            sender,
                            offset: offset as u64,
                        });
                }
            }
        }

        Self { sequences }
    }
}
