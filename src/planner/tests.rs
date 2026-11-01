use super::*;
use crate::{
    crdt::{
        Scalar,
        state::{CrdtDelta, CrdtValue, DeltaOp, Status, Tracked, status::reduce_to_statuses},
    },
    eu::{StatDbCommitter, StateDb, output::ExecutionOutput},
    namespace::Namespace,
    planner::{resolver::Checker, scheduler::ID},
    store::traits::Committer,
};

fn job(id: u64, contract: Address, callee: u64) -> Job<()> {
    Job {
        tx: (),
        id: ID::new(id),
        sender: Address::with_last_byte(id as u8).into_array(),
        nonce: 0,
        callee: Some(callee),
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
            ..ID::new(id)
        },
        key,
        gas_used: 0,
        accumulated_gas: 0,
        data: tracked.clone_with_states(snapshots),
    }
}

#[test]
fn detected_history_survives_generations_and_changes_scheduling() {
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
                .conflicts
                .iter()
                .map(|(left, right)| (left.global, right.global))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(1, 2)])
        );
        assert!(result.committable.is_empty());
        assert_eq!(
            planner.load_history(a).unwrap(),
            if round == 0 { vec![] } else { vec![b] }
        );
        assert_eq!(
            planner.load_history(b).unwrap(),
            if round == 0 { vec![] } else { vec![a] }
        );
        if round == 0 {
            assert_eq!(result.history.len(), 2);
        } else {
            assert!(result.history.is_empty());
        }
        let conflicts = planner.checker.check();
        let empty = planner.resolve(conflicts).unwrap();
        assert!(empty.conflicts.is_empty());
        assert!(empty.history.is_empty());
        drop(planner);

        let mut committer = StatDbCommitter::with_state(db.into_inner());
        committer.buffer(ID::new(0), result.history);
        committer.flush();
        db = StateDb::new(committer.into_inner());
        let mut planner = Planner::<_, u64>::new(&mut db, config);
        assert_eq!(planner.load_history(a).unwrap(), vec![b]);
        assert_eq!(planner.load_history(b).unwrap(), vec![a]);
        assert_eq!(
            planner.db.storage(a, Field::Count.slot(0)).unwrap(),
            U256::from(1)
        );
    }

    // Every function key of B inherits B's contract-level history.
    let mut planner = Planner::<_, u64>::new(&mut db, config);
    assert_eq!(
        planner.plan([job(1, a, 11), job(2, b, 21)]).unwrap().len(),
        2
    );
    let c = Address::repeat_byte(0x33);
    planner.plan([job(1, a, 11), job(3, c, 30)]).unwrap();
    planner
        .checker
        .import(vec![record(1, 7, true), record(3, 7, false)]);
    let conflicts = planner.checker.check();
    let result = planner.resolve(conflicts).unwrap();
    assert_eq!(planner.load_history(a).unwrap(), vec![b]);
    assert!(planner.load_history(c).unwrap().is_empty());
    let count = &result.history[&a].storage[&Field::Count.slot(0)];
    assert_eq!(count.original_value(), U256::from(1));
    assert_eq!(count.present_value(), U256::from(2));
    drop(planner);
    let mut committer = StatDbCommitter::with_state(db.into_inner());
    committer.buffer(ID::new(0), result.history);
    committer.flush();
    let mut planner = Planner::<_, u64>::new(StateDb::new(committer.into_inner()), config);
    assert_eq!(planner.load_history(a).unwrap(), vec![b, c]);
    assert_eq!(planner.load_history(c).unwrap(), vec![a]);
    assert_eq!(planner.db.basic(a).unwrap().unwrap().nonce, 9);
    assert_eq!(
        planner.db.storage(a, U256::from(7)).unwrap(),
        U256::from(99)
    );
}

#[test]
fn self_conflicts_constrain_different_functions_of_the_same_contract() {
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
    assert!(planner.load_history(a).unwrap().is_empty());
    let mut committer = StatDbCommitter::with_state(planner.db.into_inner());
    committer.buffer(ID::new(0), result.history);
    committer.flush();
    let mut planner = Planner::<_, u64>::new(StateDb::new(committer.into_inner()), config);
    assert_eq!(planner.load_history(a).unwrap(), vec![a]);
    assert_eq!(
        planner.plan([job(1, a, 12), job(2, a, 13)]).unwrap().len(),
        2
    );
}

#[test]
fn resolve_returns_history_with_a_read_only_database() {
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
    assert!(result.committable.is_empty());
    assert_eq!(result.violations.len(), 1);
    assert_eq!(result.history.len(), 2);
    for (contract, peer) in [(a, b), (b, a)] {
        assert_eq!(planner.db.basic(contract).unwrap(), None);
        assert!(planner.load_history(contract).unwrap().is_empty());
        let account = &result.history[&contract];
        assert!(account.is_touched());
        let count = &account.storage[&Field::Count.slot(0)];
        assert_eq!(count.original_value(), U256::ZERO);
        assert_eq!(count.present_value(), U256::from(1));
        let entry = &account.storage[&Field::Entry.slot(0)];
        assert_eq!(entry.original_value(), U256::ZERO);
        assert_eq!(entry.present_value(), U256::from_be_slice(peer.as_slice()));
    }
}

#[test]
fn shared_reads_do_not_create_history() {
    let a = Address::repeat_byte(0x11);
    let b = Address::repeat_byte(0x22);
    let mut planner = Planner::<_, u64>::new(StateDb::default(), SchedulerConfig::default());
    planner.plan([job(1, a, 10), job(2, b, 20)]).unwrap();
    planner
        .checker
        .import(vec![record(1, 7, false), record(2, 7, false)]);
    let conflicts = planner.checker.check();
    let result = planner.resolve(conflicts).unwrap();
    assert!(result.conflicts.is_empty());
    assert!(result.history.is_empty());
    assert!(planner.load_history(a).unwrap().is_empty());
    assert!(planner.load_history(b).unwrap().is_empty());
}

#[test]
fn numeric_conflicts_report_contributing_transactions() {
    let mut detector = ConflictChecker::new();
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
                    ..ID::new(id)
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
    assert!(result.committable.is_empty());
    assert_eq!(
        result
            .conflicts
            .iter()
            .map(|(left, right)| (left.global, right.global))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([(1, 3), (2, 3)])
    );
}

#[test]
fn history_keys_use_registered_storage_only_namespace() {
    use crate::eu::{ApiAddress, CallHandler, KeyCodec, U256Handler};
    let count = Field::Count.slot(0);
    let entry = Field::Entry.slot(42);
    assert_eq!(
        KeyCodec::decode(count),
        Some((Namespace::Conflict, 0, [0; 24]))
    );
    let (namespace, field, key) = KeyCodec::decode(entry).unwrap();
    assert_eq!(namespace, Namespace::Conflict);
    assert_eq!(field, 1);
    assert_eq!(&key[16..], &42_u64.to_be_bytes());
    assert_eq!(
        &count.to_be_bytes::<32>()[..7],
        &entry.to_be_bytes::<32>()[..7]
    );
    assert!(!ApiAddress::is_storage_key(count));
    assert!(!U256Handler.is_valid_slot(count));
    assert!(!U256Handler.is_valid_slot(entry));
    assert!(ApiAddress::try_from(Namespace::Conflict).is_err());
    assert_eq!(
        ApiAddress::try_from(Namespace::Scalar),
        Ok(ApiAddress::Scalar)
    );
}
