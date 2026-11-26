//! Read, write, delta, and delete operation collection shared by inspectors and native call handlers.

use std::collections::HashMap;

use crate::crdt::state::{
    Tracked,
    op::Operations,
    status::{Status, Tag},
    value::ValueSnapshots,
};
use crate::execution::key::Key;
use crate::execution::output::ExecutionOutput;
use rayon::prelude::*;

pub type RecorderOutput = Vec<ExecutionOutput<Key, Tracked<Status, Status>>>;

pub(in crate::execution) const MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER: usize = 1_024;

/// Counts state reads, writes, deltas, and delete attempts for one transaction.
pub struct Recorder {
    id: u64,
    access: HashMap<Key, Operations>,
}

impl Recorder {
    /// Creates an empty recorder for the transaction.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            access: HashMap::new(),
        }
    }

    /// Start another transaction while retaining the recorder's allocation.
    pub fn reset(&mut self, id: u64) {
        self.id = id;
        self.clear();
    }

    /// Clears counts without changing the current transaction ID.
    pub(in crate::execution) fn clear(&mut self) {
        self.access.clear();
    }

    /// Records one read of an account field or storage slot.
    pub fn log_read(&mut self, key: Key) {
        self.access.entry(key).or_default().reads.increment();
    }

    /// Records one storage write attempt.
    pub fn log_write(&mut self, key: Key) {
        self.access.entry(key).or_default().writes.increment();
    }

    /// Records one delta update attempt.
    pub fn log_delta(&mut self, key: Key) {
        self.access.entry(key).or_default().deltas.increment();
    }

    /// Records one storage clear attempt.
    pub fn log_delete(&mut self, key: Key) {
        self.access.entry(key).or_default().removes.increment();
    }

    /// Combines counts from another recorder for the same transaction.
    pub fn merge(&mut self, mut other: Self) {
        assert_eq!(
            self.id, other.id,
            "cannot merge recorders with different ids"
        );

        self.merge_reads(&mut other);
    }

    /// Takes EVM-local counts under this recorder's transaction ID.
    pub(in crate::execution) fn merge_reads(&mut self, other: &mut Self) {
        if self.access.len() < other.access.len() {
            std::mem::swap(&mut self.access, &mut other.access);
        }

        for (key, operations) in other.access.drain() {
            let current = self.access.entry(key).or_default();
            current.reads = (current.reads.count() + operations.reads.count()).into();
            current.writes = (current.writes.count() + operations.writes.count()).into();
            current.deltas = (current.deltas.count() + operations.deltas.count()).into();
            current.removes = (current.removes.count() + operations.removes.count()).into();
            current.existence_checks =
                (current.existence_checks.count() + operations.existence_checks.count()).into();
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

    /// Exports operations, grouping account fields by account, and clears the counts.
    pub fn drain(&mut self) -> RecorderOutput {
        let mut access = HashMap::<Key, Operations>::new();
        for (key, operations) in self.access.drain() {
            let key = match key {
                Key::Balance(address) | Key::Code(address) => Key::Account(address),
                key => key,
            };
            let current = access.entry(key).or_default();
            current.reads = (current.reads.count() + operations.reads.count()).into();
            current.writes = (current.writes.count() + operations.writes.count()).into();
            current.deltas = (current.deltas.count() + operations.deltas.count()).into();
            current.removes = (current.removes.count() + operations.removes.count()).into();
            current.existence_checks =
                (current.existence_checks.count() + operations.existence_checks.count()).into();
        }

        access
            .into_iter()
            .map(|(key, operations)| ExecutionOutput {
                id: crate::execution::workload::ID::new(0, self.id),
                key,
                gas_used: 0,
                accumulated_gas: 0,
                data: Tracked {
                    id: self.id,
                    value: ValueSnapshots {
                        original: Status::Tag(Tag::Unknown),
                        current: Status::Tag(Tag::Unknown),
                    },
                    operations,
                },
            })
            .collect()
    }

    pub(in crate::execution) fn should_merge_parallel(recorders: &[Recorder]) -> bool {
        let pairs = recorders.len() / 2;
        let workers = rayon::current_num_threads().min(pairs);

        if workers < 2 {
            return false;
        }

        let lesser_entries: usize = recorders
            .chunks_exact(2)
            .map(|pair| pair[0].access.len().min(pair[1].access.len()))
            .sum(); // Sum the lesser entries of each pair

        lesser_entries >= MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER * workers
    }
}

#[cfg(test)]
#[path = "tests/recorder.rs"]
mod tests;
