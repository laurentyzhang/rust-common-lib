use super::sender::{Sender, SenderNonceOffset};
use alloy_evm::revm::context_interface::Transaction;
use alloy_primitives::TxKind;
use std::collections::{BTreeMap, BTreeSet};

/// Transaction data supplied to the scheduler before planning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Job<Tx> {
    /// Original executable transaction carried through scheduling.
    pub tx: Tx,
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

impl<TxEnv: Transaction, R> Job<(TxEnv, R)> {
    /// Builds a job from an executable transaction. `callee` is the target's first
    /// 4 address bytes plus the 4-byte selector.
    pub fn new(id: u64, (tx_env, recovered): (TxEnv, R)) -> Self {
        let callee = match tx_env.kind() {
            TxKind::Call(to) => {
                let mut key = [0u8; 8];
                key[..4].copy_from_slice(&to[..4]);
                let input = tx_env.input();
                let n = input.len().min(4);
                key[4..4 + n].copy_from_slice(&input[..n]);
                Some(u64::from_le_bytes(key))
            }
            TxKind::Create => None,
        };
        Self {
            id,
            sender: tx_env.caller().0.0,
            nonce: tx_env.nonce(),
            callee,
            is_deferred: false,
            fully_parallelizable: false,
            tx: (tx_env, recovered),
        }
    }
}

/// Transactions in one sequence execute in order; sequences in a generation may run in parallel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobSequence<Tx> {
    pub jobs: Vec<Job<Tx>>,
    /// One entry per sender in this sequence, ordered by sender address.
    /// Offsets count that sender's earlier transactions within the generation.
    pub nonce_offsets: Vec<SenderNonceOffset>,
}

impl<Tx> JobSequence<Tx> {
    /// Keeps the supplied transaction order and leaves nonce offsets empty.
    /// The generation constructor calculates offsets once its layout is fixed.
    pub(super) fn new(jobs: Vec<Job<Tx>>) -> Self {
        Self {
            jobs,
            nonce_offsets: Vec::new(),
        }
    }
}

impl<Tx> Default for JobSequence<Tx> {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            nonce_offsets: Vec::new(),
        }
    }
}

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
    /// Rebuilds nonce offsets for the supplied generation without reordering jobs.
    /// For each sender in a sequence, counts that sender's jobs before its first
    /// job in the generation's nonce and ID ordering. Stores one offset per
    /// sender, including zero offsets, in sender-address order.
    pub(super) fn new(mut sequences: Vec<JobSequence<Tx>>) -> Self {
        let mut transactions_by_sender = BTreeMap::<Sender, Vec<(u64, u64, usize)>>::new();
        for (sequence_index, sequence) in sequences.iter_mut().enumerate() {
            sequence.nonce_offsets.clear();
            for job in &sequence.jobs {
                transactions_by_sender.entry(job.sender).or_default().push((
                    job.nonce,
                    job.id,
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
