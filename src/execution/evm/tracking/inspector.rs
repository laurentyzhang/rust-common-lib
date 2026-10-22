use std::collections::HashMap;

use crate::crdt::state::{
    Tracked,
    op::Operations,
    status::{Status, Tag},
    value::ValueSnapshots,
};
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

pub(in crate::execution::evm) const MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER: usize = 1_024;

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

    pub(in crate::execution::evm) fn record(&mut self, key: Key) {
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

    pub(in crate::execution::evm) fn should_merge_parallel(
        recorders: &[StateReadRecorder],
    ) -> bool {
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

            opcode::SLOAD => {
                if let Ok(slot) = interp.stack.peek(0) {
                    self.record(Key::Storage(interp.input.target_address(), slot));
                }
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
