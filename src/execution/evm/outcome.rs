use std::collections::HashMap;

use alloy_evm::block::BlockExecutionError;
use alloy_evm::revm::context_interface::result::{HaltReason, ResultAndState};

use super::{StateReads, key::Key};
use crate::{
    crdt::state::{
        Tracked,
        op::Operations,
        status::{Status, Tag},
        value::ValueSnapshots,
    },
    scheduler::ID,
};

/// The EVM output, receipt, and recorded state reads for one transaction.
pub struct TransactionOutcome<R, H = HaltReason> {
    pub result_and_state: ResultAndState<H>,
    pub receipt: R,
    pub reads: StateReads, // Combined from opcode inspection and API handlers.
}

impl<R, H> TransactionOutcome<R, H> {
    /// Merges inspector reads with state changes, keeping the state record for shared keys.
    pub fn merge_records(&mut self, transaction_id: u64) -> Vec<(Key, Tracked<Status, Status>)> {
        let mut unique_records: HashMap<Key, Tracked<Status, Status>> = HashMap::new();
        for read in self.reads.drain(..) {
            let mut tracked = read.data;
            tracked.id = transaction_id;
            unique_records.insert(read.key, tracked);
        }

        for (key, mut write) in self.delta_records(transaction_id) {
            if let Some(read) = unique_records.get(&key) {
                write.operations.reads = read.operations.reads;
            }
            unique_records.insert(key, write);
        }
        unique_records.into_iter().collect()
    }

    /// Exports REVM account and storage accesses without consuming the result.
    fn delta_records(&self, id: u64) -> Vec<(Key, Tracked<Status, Status>)> {
        let mut records = self.account_delta_records(id);
        records.extend(self.slot_delta_records(id));
        records
    }

    /// Collects account records from the result state.
    fn account_delta_records(&self, id: u64) -> Vec<(Key, Tracked<Status, Status>)> {
        let mut records = Vec::with_capacity(self.result_and_state.state.len());

        for (address, account) in &self.result_and_state.state {
            let mut operations = Operations::default();

            let original = if account.is_loaded_as_not_existing() || account.is_created() {
                Tag::Missing
            } else {
                Tag::Stripped
            };

            let current = if account.is_selfdestructed() {
                operations.removes.increment();
                Tag::Deleted
            } else if account.is_changed() || account.is_created() {
                operations.deltas.increment();
                if account.is_empty() {
                    Tag::Default
                } else {
                    Tag::Stripped
                }
            } else {
                Tag::Missing
            };

            records.push((
                Key::Account(*address),
                Tracked {
                    id,
                    value: ValueSnapshots {
                        original: Status::Tag(original),
                        current: Status::Tag(current),
                    },
                    operations,
                },
            ));
        }
        records
    }

    /// Collects storage records from the result state.
    fn slot_delta_records(&self, id: u64) -> Vec<(Key, Tracked<Status, Status>)> {
        let mut records = Vec::new();

        for (address, account) in &self.result_and_state.state {
            for (slot, value) in &account.storage {
                let mut operations = Operations::default();

                let original = if value.original_value().is_zero() {
                    Tag::Missing
                } else {
                    Tag::Stripped
                };

                let current = if !value.is_changed() {
                    Tag::Missing
                } else if value.present_value().is_zero() {
                    operations.removes.increment();
                    Tag::Deleted
                } else {
                    operations.deltas.increment();
                    Tag::Stripped
                };

                records.push((
                    Key::Storage(*address, *slot),
                    Tracked {
                        id,
                        value: ValueSnapshots {
                            original: Status::Tag(original),
                            current: Status::Tag(current),
                        },
                        operations,
                    },
                ));
            }
        }

        records
    }
}

/// A transaction result tagged with its stable global ID and scheduled location.
pub struct ScheduledTransactionResult<R, H = HaltReason> {
    pub id: ID,
    pub outcome: Result<TransactionOutcome<R, H>, BlockExecutionError>,
}
