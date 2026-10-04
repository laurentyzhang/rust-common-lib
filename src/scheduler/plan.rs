use std::collections::{BTreeMap, BTreeSet};

use super::job_resolver::{ExecutionMode, ResolvedJob};
use super::scheduler::SchedulerConfig;
use super::sender::Sender;
use super::workload::{Generation, Job, JobSequence};

type NonceKey = (u64, u64);

/// Finalized execution plan consisting of multiple generations.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecutionPlan {
    pub generations: Vec<Generation>,
}

/// Draft execution plan used during scheduling before finalization.
#[derive(Default)]
pub(super) struct DraftPlan<'a> {
    generations: Vec<DraftGeneration<'a>>,
}

#[derive(Default)]
struct DraftGeneration<'a> {
    sequences: Vec<DraftSequence<'a>>,
}

struct DraftSequence<'a> {
    transactions: Vec<DraftTransaction<'a>>,
}

struct DraftTransaction<'a> {
    job: ResolvedJob<'a>,
    is_deferred: bool,
}

impl<'a> DraftTransaction<'a> {
    /// Returns the sender address used to group jobs and compare nonce order.
    fn sender(&self) -> Sender {
        self.job.job.sender
    }

    /// Returns the nonce and transaction ID, using the ID to break equal-nonce ties.
    fn nonce_key(&self) -> NonceKey {
        (self.job.job.nonce, self.job.job.id)
    }

    /// Copies the planned deferral flag into the job and drops its resolved policy.
    fn finalize(self) -> Job {
        let mut transaction = self.job.job;
        transaction.is_deferred = self.is_deferred;
        transaction
    }
}

impl<'a> DraftSequence<'a> {
    /// Starts a sequence with one resolved job and its planned deferral flag.
    fn from_job(job: ResolvedJob<'a>, is_deferred: bool) -> Self {
        Self {
            transactions: vec![DraftTransaction { job, is_deferred }],
        }
    }

    /// Reports whether any pair of jobs across the two sequences cannot run in parallel.
    fn conflicts_with(&self, other: &Self) -> bool {
        self.transactions.iter().any(|left| {
            other
                .transactions
                .iter()
                .any(|right| !left.job.can_share_batch_with(&right.job))
        })
    }

    /// Finalizes jobs in their existing order; the generation assigns nonce offsets later.
    fn finalize(self) -> JobSequence {
        JobSequence::new(
            self.transactions
                .into_iter()
                .map(DraftTransaction::finalize)
                .collect(),
        )
    }
}

impl<'a> DraftGeneration<'a> {
    /// Iterates over every transaction, visiting sequences in their stored order.
    fn transactions(&self) -> impl Iterator<Item = &DraftTransaction<'_>> {
        self.sequences
            .iter()
            .flat_map(|sequence| &sequence.transactions)
    }

    /// Counts all transactions across this generation's sequences.
    fn job_count(&self) -> usize {
        self.sequences
            .iter()
            .map(|sequence| sequence.transactions.len())
            .sum()
    }

    /// Finds the first call to the callee as a (sequence index, transaction index) pair.
    /// Returns `None` if no callee was supplied or no existing call matches it.
    fn repeated_callee(&self, callee: Option<u64>) -> Option<(usize, usize)> {
        let callee = callee?;
        self.sequences
            .iter()
            .enumerate()
            .find_map(|(sequence_index, sequence)| {
                sequence
                    .transactions
                    .iter()
                    .enumerate()
                    .find(|(_, current)| current.job.job.callee == Some(callee))
                    .map(|(transaction_index, _)| (sequence_index, transaction_index))
            })
    }

    /// Reports whether another sequence here contains a same-sender predecessor.
    /// `src_seq_ind` selects the source sequence considered for moving; its own
    /// transactions are excluded because they move together. Order is by nonce, then ID.
    fn has_earlier_sender_transaction(&self, src_seq_ind: usize) -> bool {
        let src_seq = &self.sequences[src_seq_ind]; // candidate sequence
        self.sequences.iter().enumerate().any(|(index, remaining)| {
            index != src_seq_ind
                && remaining.transactions.iter().any(|earlier| {
                    src_seq.transactions.iter().any(|moving| {
                        earlier.sender() == moving.sender()
                            && earlier.nonce_key() < moving.nonce_key()
                    })
                })
        })
    }

    /// Checks whether appending the candidate keeps each sender's nonce span contiguous.
    /// Rejects a merge if another destination sequence contains a same-sender transaction
    /// whose nonce key falls between the first and last keys in the merged sequence.
    fn preserves_nonce_spans(&self, destination: usize, candidate: &DraftSequence<'_>) -> bool {
        let mut spans = BTreeMap::<Sender, (NonceKey, NonceKey)>::new();
        for transaction in self.sequences[destination]
            .transactions
            .iter()
            .chain(&candidate.transactions)
        {
            spans
                .entry(transaction.sender())
                .and_modify(|(first, last)| {
                    *first = (*first).min(transaction.nonce_key());
                    *last = (*last).max(transaction.nonce_key());
                })
                .or_insert_with(|| {
                    let key = transaction.nonce_key();
                    (key, key)
                });
        }

        !self.sequences.iter().enumerate().any(|(index, sequence)| {
            index != destination
                && sequence.transactions.iter().any(|transaction| {
                    spans
                        .get(&transaction.sender())
                        .is_some_and(|(first, last)| {
                            *first < transaction.nonce_key() && transaction.nonce_key() < *last
                        })
                })
        })
    }

    /// Finds a target sequence in this earlier generation for `source.sequences[source_seq_ind]`.
    /// Returns its index only when exactly one sequence conflicts with the candidate and
    /// the size, execution-mode, deferral, and sender-order checks allow the merge.
    /// Returns `None` otherwise. The whole source sequence is considered; this function
    /// only checks eligibility, while `merge_into` performs the move.
    fn find_merge_target(
        &self,
        source: &Self,
        source_seq_ind: usize,
        config: &SchedulerConfig,
    ) -> Option<usize> {
        if self.job_count() >= config.merge_threshold {
            return None;
        }

        // Preserve isolated and deferred execution boundaries.
        if self.transactions().any(|transaction| {
            transaction.job.mode == ExecutionMode::Isolated || transaction.is_deferred
        }) {
            return None;
        }
        let candidate = &source.sequences[source_seq_ind];
        if candidate.transactions.iter().any(|transaction| {
            transaction.job.mode == ExecutionMode::Isolated
                || transaction.is_deferred
                || (config.deferral_enabled && transaction.job.deferrable)
        }) {
            return None;
        }

        if source.has_earlier_sender_transaction(source_seq_ind) {
            return None;
        }

        // Serializing after exactly one conflicting sequence removes the conflict
        // without preventing the other destination sequences from running in parallel.
        let mut conflicts = self
            .sequences
            .iter()
            .enumerate()
            .filter(|(_, sequence)| candidate.conflicts_with(sequence))
            .map(|(index, _)| index);
        let destination = conflicts.next()?;
        if conflicts.next().is_some() || !self.preserves_nonce_spans(destination, candidate) {
            return None;
        }
        Some(destination)
    }

    /// Moves eligible sequences from this source generation into the preceding generation.
    /// Each sequence is removed as a whole and appended to its target in transaction order.
    /// Sequences that cannot merge stay here and continue to constrain later candidates.
    fn merge_into(&mut self, destination: &mut Self, config: &SchedulerConfig) {
        let mut source_seq_ind = 0;
        while source_seq_ind < self.sequences.len() {
            match destination.find_merge_target(self, source_seq_ind, config) {
                Some(destination_index) => {
                    let sequence = self.sequences.remove(source_seq_ind);
                    destination.sequences[destination_index]
                        .transactions
                        .extend(sequence.transactions);
                    // Removal shifts the next candidate into the same index.
                }
                None => source_seq_ind += 1,
            }
        }
    }

    /// Sorts sequences by their first transaction ID, finalizes their jobs, and computes
    /// sender nonce offsets using the completed generation layout.
    fn finalize(mut self) -> Generation {
        self.sequences
            .sort_unstable_by_key(|sequence| sequence.transactions[0].job.job.id);
        Generation::new(
            self.sequences
                .into_iter()
                .map(DraftSequence::finalize)
                .collect(),
        )
    }
}

impl<'a> DraftPlan<'a> {
    /// Appends generations for a conflict-compatible batch, splitting repeated
    /// deferrable callees when deferral is enabled and recording each job's deferred flag.
    /// A sender's later jobs stay in the same generation as its earlier jobs or a later one.
    pub(super) fn push_batch(&mut self, batch: Vec<ResolvedJob<'a>>, config: &SchedulerConfig) {
        let mut generations = Vec::<DraftGeneration<'a>>::new();
        let mut sender_floor = BTreeMap::<Sender, usize>::new();
        let mut split_sources = BTreeSet::<usize>::new();

        for job in batch {
            let sender = job.job.sender;
            let mut generation_index = sender_floor.get(&sender).copied().unwrap_or(0);
            let mut is_deferred = false;

            loop {
                while generations.len() <= generation_index {
                    generations.push(DraftGeneration::default());
                }

                let repeated = if config.deferral_enabled && job.deferrable {
                    generations[generation_index].repeated_callee(job.job.callee)
                } else {
                    None
                };

                let Some((sequence_index, transaction_index)) = repeated else {
                    generations[generation_index]
                        .sequences
                        .push(DraftSequence::from_job(job, is_deferred));
                    sender_floor.insert(sender, generation_index);
                    break;
                };

                let existing = &mut generations[generation_index].sequences[sequence_index]
                    .transactions[transaction_index];
                if existing.job.job.id > job.job.id {
                    existing.is_deferred = true;
                } else {
                    is_deferred = true;
                }
                split_sources.insert(generation_index);
                generation_index += 1;
            }
        }

        for (index, mut generation) in generations.into_iter().enumerate() {
            if config.deferral_enabled
                && !split_sources.contains(&index)
                && generation.job_count() == 1
            {
                let transaction = &mut generation.sequences[0].transactions[0];
                if transaction.job.deferrable {
                    transaction.is_deferred = true;
                }
            }
            self.push_generation(generation);
        }
    }

    /// Appends the generation only if it contains at least one sequence.
    fn push_generation(&mut self, generation: DraftGeneration<'a>) {
        if !generation.sequences.is_empty() {
            self.generations.push(generation);
        }
    }

    /// Reduces execution rounds by merging into the immediately preceding retained generation.
    /// Drops generations emptied by merging; partially merged generations remain as
    /// boundaries for subsequent moves. A zero merge threshold disables this pass.
    pub(super) fn compact(&mut self, config: &SchedulerConfig) {
        if config.merge_threshold == 0 {
            return;
        }

        let mut compacted = Vec::<DraftGeneration<'a>>::with_capacity(self.generations.len());
        for mut source in self.generations.drain(..) {
            if let Some(previous) = compacted.last_mut() {
                source.merge_into(previous, config);
            }
            // A partially merged generation stays as the next merge destination.
            if !source.sequences.is_empty() {
                compacted.push(source);
            }
        }
        self.generations = compacted;
    }

    /// Builds the public plan in generation order, finalizing each generation's jobs
    /// and sender nonce offsets after all layout changes are complete.
    pub(super) fn finalize(self) -> ExecutionPlan {
        ExecutionPlan {
            generations: self
                .generations
                .into_iter()
                .map(DraftGeneration::finalize)
                .collect(),
        }
    }
}
