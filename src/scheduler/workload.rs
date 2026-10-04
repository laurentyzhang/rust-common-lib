use super::sender::{Sender, SenderNonceOffset};
use std::collections::{BTreeMap, BTreeSet};

/// Transaction data supplied to the scheduler before planning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Job {
    pub id: u64,
    pub sender: Sender,
    pub nonce: u64,
    pub callee: Option<u64>,
    /// Set by the scheduler in the generated execution plan.
    pub is_deferred: bool,
    /// Bypass this transaction's callee restrictions, while respecting another
    /// transaction's isolated execution. Deferral remains profile-driven.
    pub fully_parallelizable: bool,
}

/// Transactions in one sequence execute in order; sequences in a generation may run in parallel.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JobSequence {
    pub transactions: Vec<Job>,
    /// One entry per sender in this sequence, ordered by sender address.
    /// Offsets count that sender's earlier transactions within the generation.
    pub nonce_offsets: Vec<SenderNonceOffset>,
}

impl JobSequence {
    /// Keeps the supplied transaction order and leaves nonce offsets empty.
    /// The generation constructor calculates offsets once its layout is fixed.
    pub(super) fn new(transactions: Vec<Job>) -> Self {
        Self {
            transactions,
            nonce_offsets: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Generation {
    pub sequences: Vec<JobSequence>,
}

impl Generation {
    /// Rebuilds nonce offsets for the supplied generation without reordering jobs.
    /// For each sender in a sequence, counts that sender's jobs before its first
    /// job in the generation's nonce and ID ordering. Stores one offset per
    /// sender, including zero offsets, in sender-address order.
    pub(super) fn new(mut sequences: Vec<JobSequence>) -> Self {
        let mut transactions_by_sender = BTreeMap::<Sender, Vec<(u64, u64, usize)>>::new();
        for (sequence_index, sequence) in sequences.iter_mut().enumerate() {
            sequence.nonce_offsets.clear();
            for transaction in &sequence.transactions {
                transactions_by_sender
                    .entry(transaction.sender)
                    .or_default()
                    .push((transaction.nonce, transaction.id, sequence_index));
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
