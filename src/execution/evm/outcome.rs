use std::collections::HashMap;

use alloy_evm::block::BlockExecutionError;
use alloy_evm::revm::context_interface::result::{HaltReason, ResultAndState};

use super::{delta_records, inspector::StateReads, key::Key};
use crate::{
    crdt::state::{Tracked, status::Status},
    scheduler::ID,
};

/// The EVM output, receipt, and recorded state reads for one transaction.
pub struct TransactionOutcome<R, H = HaltReason> {
    pub result_and_state: ResultAndState<H>,
    pub receipt: R,
    pub inspector_reads: StateReads,
}

impl<R, H> TransactionOutcome<R, H> {
    /// Merges inspector reads with state changes, keeping the state record for shared keys.
    pub fn merge_records(&mut self, transaction_id: u64) -> Vec<(Key, Tracked<Status, Status>)> {
        let mut unique_records: HashMap<Key, Tracked<Status, Status>> = HashMap::new();
        for read in self.inspector_reads.drain(..) {
            let mut tracked = read.data;
            tracked.id = transaction_id;
            unique_records.insert(read.key, tracked);
        }

        for (key, mut write) in delta_records(&self.result_and_state, transaction_id) {
            if let Some(read) = unique_records.get(&key) {
                write.operations.reads = read.operations.reads;
            }
            unique_records.insert(key, write);
        }
        unique_records.into_iter().collect()
    }
}

/// A transaction result tagged with its stable global ID and scheduled location.
pub struct ScheduledTransactionResult<R, H = HaltReason> {
    pub id: ID,
    pub outcome: Result<TransactionOutcome<R, H>, BlockExecutionError>,
}
