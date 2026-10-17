use std::collections::HashMap;

use crate::crdt::state::{Status, Tracked, op::Operations, status::Tag, value::ValueSnapshots};
use crate::execution::evm::key::Key;
use crate::execution::output::ExecutionOutput;
use alloy_evm::revm::{
    Inspector,
    bytecode::opcode,
    interpreter::{
        Interpreter,
        instructions::utility::IntoAddress,
        interpreter_types::{InputsTr, Jumps},
    },
};
use rayon::prelude::*;

pub type StateReads = Vec<ExecutionOutput<Key, Tracked<Status, Status>>>;

const MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER: usize = 1_024;

pub struct StateReadRecorder {
    id: u64,
    reads: HashMap<Key, u32>,
}

impl StateReadRecorder {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            reads: HashMap::new(),
        }
    }

    fn record(&mut self, key: Key) {
        *self.reads.entry(key).or_default() += 1;
    }

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

    pub fn drain(&mut self) -> StateReads {
        self.reads
            .drain()
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
                            original: Status::Tag(Tag::Missing),
                            current: Status::Tag(Tag::Missing),
                        },
                        operations,
                    },
                }
            })
            .collect()
    }

    fn should_merge_parallel(recorders: &[StateReadRecorder]) -> bool {
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

impl<CTX> Inspector<CTX> for StateReadRecorder {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        // The frame's bytecode has been loaded before its interpreter is initialized.
        if let Some(address) = interp.input.bytecode_address() {
            self.record(Key::Code(*address));
        }
    }

    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        match interp.bytecode.opcode() {
            opcode::BALANCE => {
                if let Ok(word) = interp.stack.peek(0) {
                    // We don't care about the result, just log the read.
                    self.record(Key::Balance(word.into_address()));
                }
            }

            opcode::SELFBALANCE => {
                // We don't care about the result, just log the read.
                self.record(Key::Balance(interp.input.target_address()));
            }

            opcode::CODECOPY => {
                if let Some(address) = interp.input.bytecode_address() {
                    self.record(Key::Code(*address));
                }
            }

            opcode::EXTCODESIZE | opcode::EXTCODECOPY | opcode::EXTCODEHASH => {
                if let Ok(word) = interp.stack.peek(0) {
                    self.record(Key::Code(word.into_address()));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, U256};

    use super::*;

    #[test]
    fn merge_sums_reads_for_the_same_key() {
        let shared = Key::Balance(Address::with_last_byte(1));
        let mut left = StateReadRecorder::new(7);
        left.record(shared.clone());
        left.record(shared.clone());
        left.record(Key::Code(Address::with_last_byte(2)));

        let mut right = StateReadRecorder::new(7);
        right.record(shared.clone());
        right.record(Key::Code(Address::with_last_byte(3)));

        left.merge(right);

        assert_eq!(left.reads.len(), 3);
        assert_eq!(left.reads[&shared], 3);
    }

    #[test]
    fn merge_all_uses_parallel_tree_for_large_recorders() {
        let recorders: Vec<_> = (0..4)
            .map(|recorder_index| {
                let mut recorder = StateReadRecorder::new(9);
                let first_slot = recorder_index * MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER;

                for slot in first_slot..first_slot + MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER {
                    recorder.record(Key::Storage(Address::ZERO, U256::from(slot)));
                }

                recorder
            })
            .collect();

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        let mut merged = StateReadRecorder::new(9);
        pool.install(|| {
            assert!(StateReadRecorder::should_merge_parallel(&recorders));
            merged.merge_all(recorders);
        });

        assert_eq!(
            merged.reads.len(),
            4 * MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER
        );
    }
}
