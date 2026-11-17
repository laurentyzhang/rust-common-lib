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
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};

    #[test]
    fn sstore_logs_writes_and_deletes_including_restored_and_reverted_slots() {
        use alloy_evm::revm::{
            Context, InspectEvm, MainBuilder, MainContext,
            context::TxEnv,
            database::InMemoryDB,
            primitives::TxKind,
            state::{AccountInfo, Bytecode},
        };

        let cases: &[(u64, &[u8], u32, u32, bool)] = &[
            (7, &[0x60, 0, 0x60, 0, 0x55, 0], 0, 1, true),
            (0, &[0x60, 0, 0x60, 0, 0x55, 0], 0, 1, true),
            (
                0,
                &[0x60, 7, 0x60, 0, 0x55, 0x60, 0, 0x60, 0, 0x55, 0],
                1,
                1,
                true,
            ),
            (
                7,
                &[0x60, 0, 0x60, 0, 0x55, 0x60, 7, 0x60, 0, 0x55, 0],
                1,
                1,
                true,
            ),
            (0, &[0x60, 7, 0x60, 0, 0x55, 0], 1, 0, true),
            (7, &[0x60, 9, 0x60, 0, 0x55, 0], 1, 0, true),
            (0, &[0x60, 0, 0x54, 0x50, 0], 0, 0, true),
            (7, &[0x60, 0, 0x54, 0x50, 0], 0, 0, true),
            (
                7,
                &[0x60, 0, 0x60, 0, 0x55, 0x60, 0, 0x60, 0, 0xfd],
                0,
                1,
                false,
            ),
            (
                7,
                &[0x60, 0, 0x60, 0, 0x55, 0x60, 0, 0x60, 0, 0x55, 0],
                0,
                2,
                true,
            ),
        ];
        let caller = Address::repeat_byte(0x11);
        let target = Address::repeat_byte(0x99);

        for &(original, code, writes, deletes, success) in cases {
            let bytecode = Bytecode::new_raw(code.to_vec().into());
            let mut db = InMemoryDB::default();
            db.insert_account_info(
                caller,
                AccountInfo::default().with_balance(U256::from(1_000_000_000u64)),
            );
            db.insert_account_info(
                target,
                AccountInfo {
                    code_hash: bytecode.hash_slow(),
                    code: Some(bytecode),
                    ..Default::default()
                },
            );
            db.insert_account_storage(target, U256::ZERO, U256::from(original))
                .unwrap();
            let mut evm = Context::mainnet()
                .with_db(db)
                .build_mainnet_with_inspector(Recorder::new(1));
            let result = evm
                .inspect_one_tx(
                    TxEnv::builder()
                        .caller(caller)
                        .kind(TxKind::Call(target))
                        .gas_limit(100_000)
                        .build()
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(result.is_success(), success);
            let operations = evm
                .inspector
                .access
                .get(&Key::Storage(target, U256::ZERO))
                .copied()
                .unwrap_or_default();
            assert_eq!(
                operations.writes.count(),
                writes,
                "original {original}, code {code:?}"
            );
            assert_eq!(
                operations.removes.count(),
                deletes,
                "original {original}, code {code:?}"
            );
        }
    }

    #[test]
    fn evm_local_counts_merge_under_the_transaction_id() {
        let key = Key::Storage(Address::ZERO, U256::from(7));
        let mut transaction = Recorder::new(17);
        transaction.log_read(key.clone());
        transaction.log_write(key.clone());
        transaction.log_delta(key.clone());
        let mut native = Recorder::new(0);
        native.log_read(key.clone());
        native.log_read(key.clone());
        native.log_write(key.clone());
        native.log_delta(key.clone());
        native.log_delta(key.clone());
        native.log_delete(key.clone());
        transaction.merge_reads(&mut native);
        assert!(native.drain().is_empty());
        let reads = transaction.drain();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].id.global, 17);
        assert_eq!(reads[0].data.id, 17);
        assert_eq!(reads[0].key, key);
        assert_eq!(reads[0].data.operations.reads.count(), 3);
        assert_eq!(reads[0].data.operations.writes.count(), 2);
        assert_eq!(reads[0].data.operations.deltas.count(), 3);
        assert_eq!(reads[0].data.operations.removes.count(), 1);
        assert!(transaction.drain().is_empty());
    }

    #[test]
    fn reset_and_drain_keep_reads_scoped_to_the_current_transaction() {
        let mut recorder = Recorder::new(1);
        recorder.log_read(Key::Account(Address::ZERO));
        recorder.log_write(Key::Storage(Address::ZERO, U256::ZERO));
        recorder.log_delete(Key::Storage(Address::ZERO, U256::ZERO));
        recorder.reset(2);
        assert!(recorder.access.is_empty());

        let key = Key::Storage(Address::ZERO, U256::from(7));
        recorder.log_read(key.clone());
        recorder.log_read(key.clone());

        let reads = recorder.drain();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].id.global, 2);
        assert_eq!(reads[0].key, key);
        assert_eq!(reads[0].data.operations.reads.count(), 2);
        assert!(recorder.drain().is_empty());
    }

    #[test]
    fn drain_preserves_write_delta_and_delete_keys_and_groups_account_reads() {
        let owner = Address::with_last_byte(1);
        let write = Key::Storage(owner, U256::from(7));
        let delete = Key::Storage(owner, U256::from(8));
        let delta = Key::Storage(owner, U256::from(9));
        let mut recorder = Recorder::new(3);
        recorder.log_read(Key::Balance(owner));
        recorder.log_read(Key::Code(owner));
        recorder.log_read(Key::Account(owner));
        recorder.log_write(write.clone());
        recorder.log_write(write.clone());
        recorder.log_delete(delete.clone());
        recorder.log_delta(delta.clone());
        let records = recorder
            .drain()
            .into_iter()
            .map(|record| (record.key, record.data.operations))
            .collect::<HashMap<_, _>>();

        assert_eq!(records.len(), 4);
        assert_eq!(records[&Key::Account(owner)].reads.count(), 3);
        assert_eq!(records[&write].reads.count(), 0);
        assert_eq!(records[&write].writes.count(), 2);
        assert_eq!(records[&write].deltas.count(), 0);
        assert_eq!(records[&write].removes.count(), 0);
        assert_eq!(records[&delete].reads.count(), 0);
        assert_eq!(records[&delete].deltas.count(), 0);
        assert_eq!(records[&delete].removes.count(), 1);
        assert_eq!(records[&delta].reads.count(), 0);
        assert_eq!(records[&delta].writes.count(), 0);
        assert_eq!(records[&delta].deltas.count(), 1);
        assert_eq!(records[&delta].removes.count(), 0);
        assert!(recorder.drain().is_empty());
    }
}
