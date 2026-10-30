use super::super::sender::{Sender, SenderNonceOffset};
use super::JobSequence;
use std::collections::{BTreeMap, BTreeSet};

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
    pub(in crate::planner::scheduler) fn assign_ids(&mut self, generation: u64) {
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
    pub(in crate::planner::scheduler) fn new(mut sequences: Vec<JobSequence<Tx>>) -> Self {
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
