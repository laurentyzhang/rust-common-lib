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
