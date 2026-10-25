//! Read collection shared by opcode inspectors and native call handlers.

use std::collections::HashMap;

use crate::crdt::state::{
    Tracked,
    op::Operations,
    status::{Status, Tag},
    value::ValueSnapshots,
};
use crate::execution::evm::key::Key;
use crate::execution::output::ExecutionOutput;
use rayon::prelude::*;

pub type StateReads = Vec<ExecutionOutput<Key, Tracked<Status, Status>>>;

pub(in crate::execution::evm) const MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER: usize = 1_024;

/// Counts state reads for one transaction, independently of their capture location.
pub struct ReadRecorder {
    id: u64,
    reads: HashMap<Key, u32>,
}

impl ReadRecorder {
    /// Creates an empty recorder for the transaction.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            reads: HashMap::new(),
        }
    }

    /// Start another transaction while retaining the recorder's allocation.
    pub fn reset(&mut self, id: u64) {
        self.id = id;
        self.reads.clear();
    }

    /// Records one read of an account field or storage slot.
    pub fn record(&mut self, key: Key) {
        *self.reads.entry(key).or_default() += 1;
    }

    /// Combines counts from another recorder for the same transaction.
    pub fn merge(&mut self, mut other: Self) {
        assert_eq!(
            self.id, other.id,
            "cannot merge recorders with different ids"
        );

        if self.reads.len() < other.reads.len() {
            std::mem::swap(&mut self.reads, &mut other.reads);
        }

        for (key, count) in other.reads {
            *self.reads.entry(key).or_default() += count;
        }
    }

    /// Merges recorder batches, using parallel workers for larger inputs.
    pub fn merge_all(&mut self, mut recorders: Vec<Self>) {
        while Self::should_merge_parallel(&recorders) {
            recorders = recorders
                .into_par_iter()
                .chunks(2)
                .map(|mut pair| {
                    let mut left = pair.remove(0);
                    if let Some(right) = pair.pop() {
                        left.merge(right);
                    }
                    left
                })
                .collect();
        }

        for recorder in recorders {
            self.merge(recorder);
        }
    }

    /// Exports reads, grouping account fields by account, and clears the counts.
    pub fn drain(&mut self) -> StateReads {
        let mut reads = HashMap::new();
        for (key, count) in self.reads.drain() {
            let key = match key {
                Key::Balance(address) | Key::Code(address) => Key::Account(address),
                key => key,
            };
            *reads.entry(key).or_insert(0_u32) += count;
        }

        reads
            .into_iter()
            .map(|(key, count)| {
                let mut operations = Operations::default();
                operations.reads = count.into();

                ExecutionOutput {
                    id: self.id,
                    key,
                    gas_used: 0,
                    data: Tracked {
                        id: self.id,
                        value: ValueSnapshots {
                            original: Status::Tag(Tag::Unknown),
                            current: Status::Tag(Tag::Unknown),
                        },
                        operations,
                    },
                }
            })
            .collect()
    }

    pub(in crate::execution::evm) fn should_merge_parallel(recorders: &[ReadRecorder]) -> bool {
        let pairs = recorders.len() / 2;
        let workers = rayon::current_num_threads().min(pairs);

        if workers < 2 {
            return false;
        }

        let lesser_entries: usize = recorders
            .chunks_exact(2)
            .map(|pair| pair[0].reads.len().min(pair[1].reads.len()))
            .sum(); // Sum the lesser entries of each pair

        lesser_entries >= MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER * workers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};

    #[test]
    fn reset_and_drain_keep_reads_scoped_to_the_current_transaction() {
        let mut recorder = ReadRecorder::new(1);
        recorder.record(Key::Account(Address::ZERO));
        recorder.reset(2);

        let key = Key::Storage(Address::ZERO, U256::from(7));
        recorder.record(key.clone());
        recorder.record(key.clone());

        let reads = recorder.drain();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].id, 2);
        assert_eq!(reads[0].key, key);
        assert_eq!(reads[0].data.operations.reads.count(), 2);
        assert!(recorder.drain().is_empty());
    }
}
