use super::{JobResult, JobSequence};
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

use crate::execution::{
    ArcologyEvmFactory, EvmStateCommitter, ParaBlockConfig, StateDb, StateOutcome, key::Key,
};
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
    /// Executes sequences in parallel, attaches verdicts, and returns accepted state changes.
    pub fn run<F, Fallback>(
        self,
        factory: &F,
        config: &ParaBlockConfig<F>,
        pool: &ThreadPool,
        checker: &mut impl Checker<Key>,
        fallback: &Fallback,
    ) -> Result<
        StateOutcome<
            Vec<JobResult<F::Receipt, <F::EvmFactory as EvmFactory>::HaltReason>>,
            EvmState,
        >,
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
        // Generate a state database for the sequences, using the provided fallback as the base.
        let gen_db = StateDb::with_fallback(EvmState::default(), Some(fallback));

        // Indexed collection preserves plan order across parallel workers.
        let results: Result<Vec<_>, Error> = pool.install(|| {
            self.sequences
                .into_par_iter()
                .map(|sequence| {
                    // Sharing the same fallback is fine because each sequence will operate on its own
                    // isolated state view and will not interfere with other sequences.
                    sequence.execute(factory, &gen_db, config.env.clone(), config.ctx.clone())
                })
                .collect()
        });
        let sequences = results?;
        for sequence in &sequences {
            for raw in &sequence.raw {
                checker.import(raw.to_records(raw.id));
            }
        }

        let verdicts = checker.check();
        let mut committer = EvmStateCommitter::with_target(gen_db.into_inner());
        let mut raws = Vec::new();
        for mut sequence in sequences {
            for raw in &mut sequence.raw {
                if let Ok(index) = verdicts.binary_search_by_key(&raw.id, |verdict| verdict.id) {
                    raw.verdict = Some(verdicts[index].clone());
                }
            }
            let outcome = sequence.finalize(fallback)?;
            if let Some(raw) = outcome.raw.first() {
                committer.buffer(raw.id, outcome.committed);
            }
            raws.extend(outcome.raw);
        }
        Ok(StateOutcome {
            raw: raws,
            committed: committer.commit(fallback)?,
        })
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
        let mut transactions_by_sender = BTreeMap::<Sender, Vec<(u64, u64, usize)>>::new();
        for (sequence_index, sequence) in sequences.iter_mut().enumerate() {
            sequence.nonce_offsets.clear();
            for job in &sequence.jobs {
                transactions_by_sender.entry(job.sender).or_default().push((
                    job.nonce,
                    job.id.global,
                    sequence_index,
                ));
            }
        }

        for (sender, mut transactions) in transactions_by_sender {
            transactions.sort_unstable_by_key(|(nonce, id, _)| (*nonce, *id));
            let mut seen_sequences = BTreeSet::new();
            for (offset, (_, _, sequence_index)) in transactions.into_iter().enumerate() {
                if seen_sequences.insert(sequence_index) {
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
