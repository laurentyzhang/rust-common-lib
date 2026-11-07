use super::*;
use crate::{
    crdt::{
        Scalar,
        state::{CrdtDelta, CrdtValue, DeltaOp, Status, Tracked, status::reduce_to_statuses},
    },
    execution::{EvmStateCommitter, StateDb, output::ExecutionOutput},
    namespace::Namespace,
    planner::{
        meta::property::Property,
        resolver::{CONFLICT_META_ID, Checker, ConflictResult, accepted_ids, rejected_ids},
        scheduler::{ID, ScheduleKey},
    },
    store::traits::Committer,
};
use alloy_evm::revm::state::{Account, EvmState, EvmStorageSlot};
use alloy_primitives::U256;

fn callee(contract: Address, selector: u32) -> u64 {
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&contract[..4]);
    bytes[4..].copy_from_slice(&selector.to_be_bytes());
    u64::from_le_bytes(bytes)
}

fn job(id: u64, contract: Address, selector: u32) -> Job<()> {
    Job {
        tx: (),
        id: ID::new(0, id),
        sender: Address::with_last_byte(id as u8).into_array(),
        nonce: 0,
        callee: Some(callee(contract, selector)),
        contract: Some(contract),
        is_deferred: false,
        fully_parallelizable: false,
    }
}

fn record(id: u64, key: u64, remove: bool) -> ExecutionOutput<u64, Tracked<Status, Status>> {
    let mut tracked = Tracked::new_owned_existing(CrdtValue::from(Scalar::<U256>::default()), id);
    if remove {
        tracked.remove().unwrap();
    } else {
        assert!(tracked.get().is_some());
    }
    let snapshots = reduce_to_statuses(&tracked.value);
    ExecutionOutput {
        id: ID {
            sequence: id,
            ..ID::new(0, id)
        },
        key,
        gas_used: 0,
        accumulated_gas: 0,
        data: tracked.clone_with_states(snapshots),
    }
}

#[test]
fn contract_metadata_survives_generations_and_keeps_functions_separate() {
    let a = Address::repeat_byte(0x11);
    let b = Address::repeat_byte(0x22);
    let config = SchedulerConfig {
        merge_threshold: 0,
        ..Default::default()
    };
    let mut account = Account::default().with_touched_mark();
    account.info.nonce = 9;
    account.storage.insert(
        U256::from(7),
        EvmStorageSlot::new(U256::from(99), Default::default()),
    );
    let mut db = StateDb::new(EvmState::from_iter([(a, account)]));
    for round in 0..2 {
        let mut planner = Planner::<_, u64>::new(&mut db, config);
        let plan = planner.plan([job(1, a, 10), job(2, b, 20)]).unwrap();
        assert_eq!(plan.len(), if round == 0 { 1 } else { 2 });
        planner
            .checker
            .import(vec![record(1, 7, true), record(2, 7, false)]);
        let conflicts = planner.checker.check();
        let result = planner.resolve(conflicts).unwrap();
        assert_eq!(
            result
                .involved_pairs
                .iter()
                .map(|(left, right)| (left.global, right.global))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(1, 2)])
        );
        assert_eq!(result.conflict_meta.0, CONFLICT_META_ID);
        assert_eq!(
            accepted_ids(&result.verdicts)
                .into_iter()
                .collect::<BTreeSet<_>>(),
            if round == 0 {
                BTreeSet::from([record(1, 7, true).id, CONFLICT_META_ID])
            } else {
                BTreeSet::from([record(1, 7, true).id])
            }
        );
        assert_eq!(
            planner.load_conflicts(a, callee(a, 10)).unwrap().1,
            if round == 0 {
                BTreeSet::new()
            } else {
                BTreeSet::from([callee(b, 20)])
            }
        );
        assert_eq!(
            planner.load_conflicts(b, callee(b, 20)).unwrap().1,
            if round == 0 {
                BTreeSet::new()
            } else {
                BTreeSet::from([callee(a, 10)])
            }
        );
        assert_eq!(result.conflict_meta.1.len(), if round == 0 { 2 } else { 0 });
        let conflicts = planner.checker.check();
        let empty = planner.resolve(conflicts).unwrap();
        assert!(empty.involved_pairs.is_empty());
        assert!(empty.conflict_meta.1.is_empty());
        assert!(empty.verdicts.is_empty());
        drop(planner);

        let mut committer = EvmStateCommitter::with_target(db.into_inner());
        committer.buffer(result.conflict_meta.0, result.conflict_meta.1);
        committer.stage(accepted_ids(&result.verdicts));
        committer.flush();
        db = StateDb::new(committer.into_inner());
        let mut planner = Planner::<_, u64>::new(&mut db, config);
        assert_eq!(
            planner.load_conflicts(a, callee(a, 10)).unwrap().1,
            BTreeSet::from([callee(b, 20)])
        );
        assert_eq!(
            planner.load_conflicts(b, callee(b, 20)).unwrap().1,
            BTreeSet::from([callee(a, 10)])
        );
        assert_eq!(
            planner
                .db
                .storage(a, Property::ConflictCount.slot(a, callee(a, 10), 0))
                .unwrap(),
            U256::from(1)
        );
    }

    let mut planner = Planner::<_, u64>::new(&mut db, config);
    assert_eq!(
        planner.plan([job(1, a, 11), job(2, b, 21)]).unwrap().len(),
        1
    );
    let c = Address::repeat_byte(0x33);
    planner.plan([job(1, a, 10), job(3, c, 30)]).unwrap();
    planner
        .checker
        .import(vec![record(1, 7, true), record(3, 7, false)]);
    let conflicts = planner.checker.check();
    let result = planner.resolve(conflicts).unwrap();
    assert_eq!(
        planner.load_conflicts(a, callee(a, 10)).unwrap().1,
        BTreeSet::from([callee(b, 20)])
    );
    assert!(
        planner
            .load_conflicts(c, callee(c, 30))
            .unwrap()
            .1
            .is_empty()
    );
    let count =
        &result.conflict_meta.1[&a].storage[&Property::ConflictCount.slot(a, callee(a, 10), 0)];
    assert_eq!(count.original_value(), U256::from(1));
    assert_eq!(count.present_value(), U256::from(2));
    drop(planner);

    let mut committer = EvmStateCommitter::with_target(db.into_inner());
    committer.buffer(result.conflict_meta.0, result.conflict_meta.1);
    committer.stage(accepted_ids(&result.verdicts));
    committer.flush();
    let mut planner = Planner::<_, u64>::new(StateDb::new(committer.into_inner()), config);
    assert_eq!(
        planner.load_conflicts(a, callee(a, 10)).unwrap().1,
        BTreeSet::from([callee(b, 20), callee(c, 30)])
    );
    assert_eq!(
        planner.load_conflicts(c, callee(c, 30)).unwrap().1,
        BTreeSet::from([callee(a, 10)])
    );
    assert_eq!(planner.db.basic(a).unwrap().unwrap().nonce, 9);
    assert_eq!(
        planner.db.storage(a, U256::from(7)).unwrap(),
        U256::from(99)
    );
}

#[test]
fn same_contract_metadata_keeps_each_function_pair_separate() {
    let a = Address::repeat_byte(0x11);
    let config = SchedulerConfig {
        merge_threshold: 0,
        ..Default::default()
    };
    let mut planner = Planner::<_, u64>::new(StateDb::default(), config);
    planner.plan([job(1, a, 10), job(2, a, 11)]).unwrap();
    planner
        .checker
        .import(vec![record(1, 7, true), record(2, 7, false)]);
    let conflicts = planner.checker.check();
    let result = planner.resolve(conflicts).unwrap();
    assert!(
        planner
            .load_conflicts(a, callee(a, 10))
            .unwrap()
            .1
            .is_empty()
    );

    let mut committer = EvmStateCommitter::with_target(planner.db.into_inner());
    committer.buffer(result.conflict_meta.0, result.conflict_meta.1);
    committer.stage(accepted_ids(&result.verdicts));
    committer.flush();
    let mut planner = Planner::<_, u64>::new(StateDb::new(committer.into_inner()), config);
    assert_eq!(
        planner.load_conflicts(a, callee(a, 10)).unwrap().1,
        BTreeSet::from([callee(a, 11)])
    );
    assert_eq!(
        planner.load_conflicts(a, callee(a, 11)).unwrap().1,
        BTreeSet::from([callee(a, 10)])
    );
    assert_eq!(
        planner.plan([job(1, a, 12), job(2, a, 13)]).unwrap().len(),
        1
    );
    assert_eq!(
        planner.plan([job(1, a, 10), job(2, a, 11)]).unwrap().len(),
        2
    );
}

#[test]
fn resolve_returns_metadata_with_a_read_only_database() {
    let a = Address::repeat_byte(0x11);
    let b = Address::repeat_byte(0x22);
    let db: Box<dyn Database<Error = std::convert::Infallible>> = Box::new(StateDb::default());
    let mut planner = Planner::<_, u64>::new(db, SchedulerConfig::default());
    planner.plan([job(1, a, 10), job(2, b, 20)]).unwrap();
    planner
        .checker
        .import(vec![record(1, 7, true), record(2, 7, false)]);
    let conflicts = planner.checker.check();
    let result = planner.resolve(conflicts).unwrap();
    assert_eq!(
        accepted_ids(&result.verdicts)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([record(1, 7, true).id, CONFLICT_META_ID])
    );
    assert_eq!(rejected_ids(&result.verdicts).len(), 1);
    assert_eq!(result.conflict_meta.1.len(), 2);
    for (contract, selector, peer) in [(a, 10, callee(b, 20)), (b, 20, callee(a, 10))] {
        let host = callee(contract, selector);
        assert_eq!(planner.db.basic(contract).unwrap(), None);
        assert!(planner.load_conflicts(contract, host).unwrap().1.is_empty());
        let account = &result.conflict_meta.1[&contract];
        assert!(account.is_touched());
        let count = &account.storage[&Property::ConflictCount.slot(contract, host, 0)];
        assert_eq!(count.original_value(), U256::ZERO);
        assert_eq!(count.present_value(), U256::from(1));
        let entry = &account.storage[&Property::ConflictIndex.slot(contract, host, 0)];
        assert_eq!(entry.original_value(), U256::ZERO);
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&peer.to_le_bytes());
        assert_eq!(entry.present_value(), U256::from_be_bytes(bytes));
    }
}

#[test]
fn conflict_metadata_uses_the_normal_commit_path_with_the_reserved_id() {
    let a = Address::repeat_byte(0x11);
    let b = Address::repeat_byte(0x22);
    let mut planner = Planner::<_, u64>::new(StateDb::default(), SchedulerConfig::default());
    planner
        .plan([job(1, a, 10), job(2, b, 20), job(3, a, 11)])
        .unwrap();
    planner.checker.import(vec![
        record(1, 7, true),
        record(2, 7, false),
        record(3, 9, false),
    ]);
    let conflicts = planner.checker.check();
    let accepted_id = conflicts
        .gas_lookup
        .keys()
        .find(|key| key.0.global == 3)
        .unwrap()
        .0;
    let mut result = planner.resolve(conflicts).unwrap();
    let meta_id = result.conflict_meta.0;
    assert_eq!(meta_id, CONFLICT_META_ID);
    assert_eq!(meta_id.global, u64::MAX);
    assert_eq!(
        accepted_ids(&result.verdicts)
            .into_iter()
            .map(|id| id.global)
            .collect::<Vec<_>>(),
        vec![1, 3, u64::MAX]
    );
    assert!(!result.gas_lookup.contains_key(&ScheduleKey(meta_id)));
    assert!(
        result
            .cumulative_gas
            .iter()
            .flatten()
            .all(|(id, _)| *id != meta_id)
    );

    let deferred = Property::DeferredCount.slot(a, callee(a, 11), 0);
    let mut accepted = Account::default().with_touched_mark();
    accepted.storage.insert(
        deferred,
        EvmStorageSlot::new_changed(U256::ZERO, U256::from(2), Default::default()),
    );
    let mut rejected = Account::default().with_touched_mark();
    rejected.storage.insert(
        U256::from(7),
        EvmStorageSlot::new_changed(U256::ZERO, U256::from(99), Default::default()),
    );
    let mut committer = EvmStateCommitter::new();
    committer.buffer(accepted_id, EvmState::from_iter([(a, accepted)]));
    committer.buffer(record(2, 7, false).id, EvmState::from_iter([(a, rejected)]));
    committer.buffer(meta_id, std::mem::take(&mut result.conflict_meta.1));
    committer.stage(accepted_ids(&result.verdicts));
    committer.flush();

    let mut db = StateDb::new(committer.into_inner());
    assert_eq!(db.storage(a, deferred).unwrap(), U256::from(2));
    assert_eq!(db.storage(a, U256::from(7)).unwrap(), U256::ZERO);
    for (host, function) in [(a, callee(a, 10)), (b, callee(b, 20))] {
        assert_eq!(
            db.storage(host, Property::ConflictCount.slot(host, function, 0))
                .unwrap(),
            U256::from(1)
        );
    }
    assert_eq!(
        rejected_ids(&result.verdicts)
            .into_iter()
            .map(|id| id.global)
            .collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn shared_reads_do_not_create_contract_metadata() {
    let a = Address::repeat_byte(0x11);
    let b = Address::repeat_byte(0x22);
    let mut planner = Planner::<_, u64>::new(StateDb::default(), SchedulerConfig::default());
    planner.plan([job(1, a, 10), job(2, b, 20)]).unwrap();
    planner
        .checker
        .import(vec![record(1, 7, false), record(2, 7, false)]);
    let conflicts = planner.checker.check();
    let result = planner.resolve(conflicts).unwrap();
    assert!(result.involved_pairs.is_empty());
    assert!(result.conflict_meta.1.is_empty());
    assert!(
        planner
            .load_conflicts(a, callee(a, 10))
            .unwrap()
            .1
            .is_empty()
    );
    assert!(
        planner
            .load_conflicts(b, callee(b, 20))
            .unwrap()
            .1
            .is_empty()
    );
}

#[test]
fn numeric_conflicts_report_contributing_transactions() {
    let mut detector = ConflictDetector::new();
    let records = (1..=3)
        .map(|id| {
            let value = Scalar::<U256>::new(U256::ZERO, U256::from(5)).unwrap();
            let mut tracked = Tracked::new_owned_existing(CrdtValue::from(value), id);
            tracked
                .add_delta(CrdtDelta::U256(DeltaOp::Add(U256::from(2))))
                .unwrap();
            let snapshots = reduce_to_statuses(&tracked.value);
            ExecutionOutput {
                id: ID {
                    sequence: id,
                    ..ID::new(0, id)
                },
                key: 7_u64,
                gas_used: 0,
                accumulated_gas: 0,
                data: tracked.clone_with_states(snapshots),
            }
        })
        .collect();
    detector.import(records);
    let result = detector.check();
    assert_eq!(
        accepted_ids(&result.verdicts)
            .into_iter()
            .map(|id| id.global)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        result
            .involved_pairs
            .iter()
            .map(|(left, right)| (left.global, right.global))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([(1, 3), (2, 3)])
    );
}

#[test]
fn contract_metadata_keys_encode_field_selector_index_and_host() {
    use super::meta::MetaKeyCodec;
    use crate::execution::{ApiAddress, CallHandler, U256Handler};
    let host = Address::repeat_byte(0x11);
    let function = callee(host, 0x12345678);
    let count = Property::ConflictCount.slot(host, function, 0);
    let entry = Property::ConflictIndex.slot(host, function, 42);
    let (field, selector, index, host_prefix) = MetaKeyCodec::decode(count).unwrap();
    assert_eq!(field, 1);
    assert_eq!(selector, 0x12345678_u32.to_be_bytes());
    assert_eq!(index, 0);
    assert_eq!(host_prefix, host[..16]);
    let (field, selector, index, host_prefix) = MetaKeyCodec::decode(entry).unwrap();
    assert_eq!(field, 2);
    assert_eq!(selector, 0x12345678_u32.to_be_bytes());
    assert_eq!(index, 42);
    assert_eq!(host_prefix, host[..16]);
    assert_eq!(
        &count.to_be_bytes::<32>()[..7],
        &entry.to_be_bytes::<32>()[..7]
    );
    assert_eq!(Property::Reserved as u8, 0);
    assert_eq!(Property::DeferredCount as u8, 3);
    assert_eq!(Property::DeferredIndex as u8, 4);
    let deferred = Property::DeferredIndex.slot(host, function, 42);
    assert_eq!(
        MetaKeyCodec::decode(deferred).unwrap(),
        (4, selector, index, host_prefix)
    );
    assert!(!ApiAddress::is_storage_key(count));
    assert!(!U256Handler.is_valid_slot(count));
    assert!(!U256Handler.is_valid_slot(entry));
    assert!(ApiAddress::try_from(Namespace::Meta).is_err());
    assert_eq!(ApiAddress::try_from(Namespace::U256), Ok(ApiAddress::U256));
}

#[test]
fn packed_metadata_appends_across_partial_and_full_slots() {
    let a = Address::repeat_byte(0x11);
    let b = Address::repeat_byte(0x22);
    let mut planner = Planner::<_, u64>::new(StateDb::default(), SchedulerConfig::default());
    let mut jobs = vec![job(1, a, 10)];
    jobs.extend((20..23).map(|selector| job(u64::from(selector), b, selector)));
    planner.plan(jobs).unwrap();
    let mut result = ConflictResult::default();
    result.involved_pairs = (20..23).map(|id| (ID::new(0, 1), ID::new(0, id))).collect();
    let result = planner.resolve(result).unwrap();
    let mut committer = EvmStateCommitter::with_target(planner.db.into_inner());
    committer.buffer(result.conflict_meta.0, result.conflict_meta.1);
    committer.stage(accepted_ids(&result.verdicts));
    committer.flush();

    let mut planner = Planner::<_, u64>::new(
        StateDb::new(committer.into_inner()),
        SchedulerConfig::default(),
    );
    let mut jobs = vec![job(1, a, 10)];
    jobs.extend((20..26).map(|selector| job(u64::from(selector), b, selector)));
    planner.plan(jobs).unwrap();
    let mut result = ConflictResult::default();
    result.involved_pairs = (20..26).map(|id| (ID::new(0, 1), ID::new(0, id))).collect();
    let result = planner.resolve(result).unwrap();
    let function = callee(a, 10);
    let count = &result.conflict_meta.1[&a].storage[&Property::ConflictCount.slot(a, function, 0)];
    assert_eq!(count.original_value(), U256::from(3));
    assert_eq!(count.present_value(), U256::from(6));
    let first = &result.conflict_meta.1[&a].storage[&Property::ConflictIndex.slot(a, function, 0)];
    let mut expected = [0; 32];
    for (index, selector) in (20..23).enumerate() {
        expected[index * 8..index * 8 + 8].copy_from_slice(&callee(b, selector).to_le_bytes());
    }
    assert_eq!(first.original_value(), U256::from_be_bytes(expected));
    expected[24..].copy_from_slice(&callee(b, 23).to_le_bytes());
    assert_eq!(first.present_value(), U256::from_be_bytes(expected));
    let second = &result.conflict_meta.1[&a].storage[&Property::ConflictIndex.slot(a, function, 1)];
    assert_eq!(second.original_value(), U256::ZERO);
    let mut expected = [0; 32];
    expected[..8].copy_from_slice(&callee(b, 24).to_le_bytes());
    expected[8..16].copy_from_slice(&callee(b, 25).to_le_bytes());
    assert_eq!(second.present_value(), U256::from_be_bytes(expected));

    let mut committer = EvmStateCommitter::with_target(planner.db.into_inner());
    committer.buffer(result.conflict_meta.0, result.conflict_meta.1);
    committer.stage(accepted_ids(&result.verdicts));
    committer.flush();
    let mut planner = Planner::<_, u64>::new(
        StateDb::new(committer.into_inner()),
        SchedulerConfig::default(),
    );
    assert_eq!(
        planner.load_conflicts(a, function).unwrap().1,
        (20..26).map(|selector| callee(b, selector)).collect()
    );
    for selector in 20..26 {
        assert_eq!(
            planner.load_conflicts(b, callee(b, selector)).unwrap().1,
            BTreeSet::from([function])
        );
    }
    assert!(
        planner
            .load_conflicts(a, callee(a, 11))
            .unwrap()
            .1
            .is_empty()
    );
}

#[test]
fn max_conflict_count_means_sequential_only() {
    let a = Address::repeat_byte(0x11);
    let b = Address::repeat_byte(0x22);
    let function = callee(a, 10);
    let slot = Property::ConflictCount.slot(a, function, 0);
    let mut account = Account::default().with_touched_mark();
    account
        .storage
        .insert(slot, EvmStorageSlot::new(U256::MAX, Default::default()));
    let mut planner = Planner::<_, u64>::new(
        StateDb::new(EvmState::from_iter([(a, account)])),
        SchedulerConfig {
            merge_threshold: 0,
            ..Default::default()
        },
    );
    assert_eq!(
        planner.load_conflicts(a, function).unwrap(),
        (true, BTreeSet::new())
    );
    assert_eq!(
        planner.plan([job(1, a, 10), job(2, b, 20)]).unwrap().len(),
        2
    );
    assert_eq!(
        planner.plan([job(1, a, 11), job(2, b, 20)]).unwrap().len(),
        1
    );
    let mut result = ConflictResult::default();
    planner.plan([job(1, a, 10), job(2, b, 20)]).unwrap();
    result.involved_pairs.insert((ID::new(0, 1), ID::new(0, 2)));
    let result = planner.resolve(result).unwrap();
    assert!(!result.conflict_meta.1.contains_key(&a));
    assert_eq!(planner.db.storage(a, slot).unwrap(), U256::MAX);
}

#[test]
fn metadata_verdict_uses_the_checked_block_and_generation() {
    let a = Address::repeat_byte(0x11);
    let b = Address::repeat_byte(0x22);
    let mut planner = Planner::<_, u64>::new(StateDb::default(), SchedulerConfig::default());
    let mut jobs = [job(1, a, 10), job(2, b, 20)];
    for job in &mut jobs {
        job.id.block = 42;
    }
    planner.plan(jobs).unwrap();
    let mut records = [record(1, 7, true), record(2, 7, false)];
    for record in &mut records {
        record.id.block = 42;
        record.id.generation = 3;
    }
    planner.checker.import(records.into());
    let conflicts = planner.checker.check();
    let result = planner.resolve(conflicts).unwrap();
    let meta_id = ID {
        block: 42,
        generation: 3,
        ..CONFLICT_META_ID
    };
    assert_eq!(result.conflict_meta.0, meta_id);
    assert!(accepted_ids(&result.verdicts).contains(&meta_id));
    assert_eq!(result.verdicts.last().unwrap().id, meta_id);
    assert!(result.verdicts.last().unwrap().rejection.is_none());
}
