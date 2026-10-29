use super::{AccountStorage, EvmStateAccumulator, StateDb};
use crate::execution::evm::{ArcologyEvmFactory, ArcologyTx, CallRouter, ReadRecorder, key::Key};
use crate::store::traits::Committer;
use alloy_evm::{
    Evm, EvmEnv, EvmFactory,
    revm::{
        Database,
        context::TxEnv,
        state::{Account, AccountInfo, Bytecode, EvmState, EvmStorageSlot, TransactionId},
    },
};
use alloy_primitives::{Address, Bytes, TxKind, U256};

fn state(balance: u64, slots: &[(u64, u64)]) -> EvmState {
    let mut account = Account::default();
    account.info.balance = U256::from(balance);
    account.mark_touch();
    for &(key, value) in slots {
        account.storage.insert(
            U256::from(key),
            EvmStorageSlot::new(U256::from(value), TransactionId::ZERO),
        );
    }
    EvmState::from_iter([(Address::ZERO, account)])
}

#[test]
fn commit_preserves_order_and_the_unpaired_fifth_state() {
    let mut accumulator = EvmStateAccumulator::new(state(0, &[(99, 99)]));
    for id in [5, 1, 4, 2, 3] {
        accumulator.buffer(id, state(id, &[(0, id * 10), (id, id)]));
    }

    accumulator.flush();
    let cache = accumulator.into_inner();

    let account = &cache[&Address::ZERO];
    assert_eq!(account.info.balance, U256::from(5));
    assert_eq!(account.storage.len(), 7);
    assert_eq!(account.storage[&U256::ZERO].present_value(), U256::from(50));
    for id in 1..=5_u64 {
        assert_eq!(
            account.storage[&U256::from(id)].present_value(),
            U256::from(id)
        );
    }
}

#[test]
fn pairwise_commit_preserves_creation_across_groups() {
    let mut accumulator = EvmStateAccumulator::new(state(0, &[(99, 99)]));
    for id in 1..=5_u64 {
        let mut incoming = state(id, &[(id, id)]);
        if id == 3 {
            incoming.get_mut(&Address::ZERO).unwrap().mark_created();
        }
        accumulator.buffer(id, incoming);
    }

    accumulator.flush();
    let cache = accumulator.into_inner();

    let account = &cache[&Address::ZERO];
    assert!(account.is_created());
    assert_eq!(account.info.balance, U256::from(5));
    assert_eq!(account.storage.len(), 3);
    for id in 3..=5_u64 {
        assert_eq!(
            account.storage[&U256::from(id)].present_value(),
            U256::from(id)
        );
    }
}

#[test]
fn pairwise_commit_retains_slot_deletion_and_allows_a_later_write() {
    for rewrite in [false, true] {
        let mut accumulator = EvmStateAccumulator::new(state(0, &[(7, 7), (99, 99)]));
        accumulator.buffer(1, state(1, &[(7, 11)]));
        let mut deletion = state(2, &[(7, 0)]);
        deletion.get_mut(&Address::ZERO).unwrap().storage.insert(
            U256::from(7),
            EvmStorageSlot::new_changed(U256::from(11), U256::ZERO, TransactionId::ZERO),
        );
        accumulator.buffer(2, deletion);
        accumulator.buffer(3, EvmState::default());
        accumulator.buffer(4, EvmState::default());
        accumulator.buffer(
            5,
            if rewrite {
                state(5, &[(7, 23)])
            } else {
                EvmState::default()
            },
        );

        accumulator.flush();
        let cache = accumulator.into_inner();

        let account = &cache[&Address::ZERO];
        let slot = &account.storage[&U256::from(7)];
        assert_eq!(
            slot.present_value(),
            U256::from(if rewrite { 23 } else { 0 })
        );
        if !rewrite {
            assert_eq!(slot.original_value(), U256::from(11));
        }
        assert_eq!(
            account.storage[&U256::from(99)].present_value(),
            U256::from(99)
        );
    }
}

#[test]
fn pairwise_commit_replaces_account_with_its_deletion_record() {
    let mut accumulator = EvmStateAccumulator::new(state(0, &[(99, 99)]));
    accumulator.buffer(1, state(1, &[(1, 1)]));
    accumulator.buffer(2, state(2, &[(2, 2)]));
    let mut deletion = state(0, &[(7, 0)]);
    deletion
        .get_mut(&Address::ZERO)
        .unwrap()
        .mark_selfdestruct();
    let expected = deletion.clone();
    accumulator.buffer(3, deletion);
    accumulator.buffer(4, EvmState::default());
    accumulator.buffer(5, EvmState::default());

    accumulator.flush();
    let cache = accumulator.into_inner();

    assert_eq!(cache, expected);
}

#[test]
fn recreation_replaces_an_earlier_account_deletion() {
    let mut accumulator = EvmStateAccumulator::new(state(0, &[(99, 99)]));
    let mut deletion = state(0, &[(1, 0)]);
    deletion
        .get_mut(&Address::ZERO)
        .unwrap()
        .mark_selfdestruct();
    accumulator.buffer(1, deletion);
    let mut creation = state(2, &[(2, 2)]);
    creation.get_mut(&Address::ZERO).unwrap().mark_created();
    accumulator.buffer(2, creation);
    accumulator.buffer(3, state(3, &[(3, 3)]));
    accumulator.buffer(4, state(4, &[(4, 4)]));
    accumulator.buffer(5, state(5, &[(5, 5)]));

    accumulator.flush();
    let cache = accumulator.into_inner();

    let account = &cache[&Address::ZERO];
    assert!(!account.is_selfdestructed());
    assert!(account.is_created());
    assert_eq!(account.storage.len(), 4);
    for id in 2..=5_u64 {
        assert_eq!(
            account.storage[&U256::from(id)].present_value(),
            U256::from(id)
        );
    }
}

#[test]
fn stage_filters_rejected_states_and_buffer_replaces_duplicate_ids() {
    let mut accumulator = EvmStateAccumulator::default();
    accumulator.buffer(1, state(1, &[(1, 1)]));
    accumulator.buffer(2, state(2, &[(2, 2)]));
    accumulator.buffer(2, state(20, &[(2, 20)]));
    accumulator.buffer(3, state(3, &[(3, 3)]));
    accumulator.stage(vec![1, 2]);

    accumulator.flush();
    let cache = accumulator.into_inner();

    let account = &cache[&Address::ZERO];
    assert_eq!(account.info.balance, U256::from(20));
    assert_eq!(account.storage.len(), 2);
    assert_eq!(
        account.storage[&U256::from(2)].present_value(),
        U256::from(20)
    );
}

#[test]
fn empty_commits_preserve_cache_and_later_commits_keep_previous_slots() {
    let cache = state(0, &[(99, 99)]);
    let original = cache.clone();
    let mut accumulator = EvmStateAccumulator::new(cache);
    accumulator.flush();
    let cache = accumulator.into_inner();
    assert_eq!(cache, original);

    let mut accumulator = EvmStateAccumulator::new(cache);
    accumulator.buffer(1, state(1, &[(1, 1)]));
    accumulator.flush();
    // A later batch with a lower ID must not replay states from the first batch.
    accumulator.buffer(0, state(2, &[(2, 2)]));
    accumulator.flush();
    let cache = accumulator.into_inner();
    let committed = cache.clone();
    assert_eq!(
        cache.get(&Address::ZERO).unwrap().info.balance,
        U256::from(2)
    );
    let mut accumulator = EvmStateAccumulator::new(cache);
    accumulator.flush();
    let cache = accumulator.into_inner();
    assert_eq!(cache, committed);
    assert_eq!(cache[&Address::ZERO].storage.len(), 3);
}

#[test]
fn into_evm_state_returns_accumulated_state() {
    let mut accumulator = EvmStateAccumulator::new(state(1, &[(1, 10)]));
    accumulator.buffer(2, state(2, &[(2, 20)]));

    accumulator.flush();
    let cache = accumulator.into_inner();
    assert_eq!(cache, state(2, &[(1, 10), (2, 20)]));
    assert!(EvmState::default().is_empty());
}

#[test]
fn into_inner_excludes_rejected_and_unflushed_states() {
    let initial = state(1, &[(1, 10)]);
    let mut accumulator = EvmStateAccumulator::new(initial.clone());
    accumulator.buffer(2, state(2, &[(2, 20)]));
    accumulator.stage(Vec::new());
    accumulator.flush();
    accumulator.buffer(3, state(3, &[(3, 30)]));

    assert_eq!(accumulator.into_inner(), initial);
}

#[test]
fn commit_writes_account_and_storage_to_accumulated_state() {
    let address = Address::repeat_byte(0x11);
    let slot = U256::from(7);
    let storage_value = U256::from(42);
    let info = AccountInfo {
        balance: U256::from(100),
        nonce: 3,
        ..AccountInfo::default()
    };
    let account = Account::from(info.clone())
        .with_storage(
            [(
                slot,
                EvmStorageSlot::new_changed(U256::ZERO, storage_value, Default::default()),
            )]
            .into_iter(),
        )
        .with_touched_mark();
    let mut changes = EvmState::default();
    changes.insert(address, account);

    let mut accumulator = EvmStateAccumulator::default();
    accumulator.buffer(9, changes);
    accumulator.flush();
    let mut db = StateDb::new(accumulator.into_inner());

    assert_eq!(db.basic(address).unwrap(), Some(info));
    assert_eq!(db.storage(address, slot).unwrap(), storage_value);
    assert_eq!(db.into_evm_state().len(), 1);
}

#[test]
fn accumulated_state_and_recorder_track_balance_reads() {
    let caller = Address::with_last_byte(0x11);
    let contract = Address::with_last_byte(0x22);
    let external = Address::with_last_byte(0x33);
    // Read the external balance twice, then read the contract's own balance.
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x60, 0x33, 0x31, 0x50, 0x60, 0x33, 0x31, 0x50, 0x47, 0x50, 0x00,
    ]));
    let account = AccountInfo {
        code_hash: code.hash_slow(),
        code: Some(code),
        ..Default::default()
    };

    let mut state = EvmState::default();
    state.insert(contract, Account::from(account));

    let factory = ArcologyEvmFactory::try_new(CallRouter::new()).unwrap();
    let recorder = ReadRecorder::new(2);
    let mut evm =
        factory.create_evm_with_inspector(StateDb::new(state), EvmEnv::default(), recorder);
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller,
            gas_limit: 500_000,
            gas_price: 0,
            kind: TxKind::Call(contract),
            ..Default::default()
        }))
        .unwrap();
    assert!(result.result.is_success());

    let (_, recorder, _) = evm.components_mut();
    let reads = recorder.drain();
    let keys: Vec<_> = reads.iter().map(|read| read.key.clone()).collect();

    assert!(keys.contains(&Key::Account(contract)));
    assert!(keys.contains(&Key::Account(external)));
    assert!(!keys.contains(&Key::Balance(external)));
    assert!(!keys.contains(&Key::Code(contract)));
    let external_read = reads
        .iter()
        .find(|read| read.key == Key::Account(external))
        .unwrap();
    assert_eq!(external_read.data.operations.reads.count(), 2);
    let contract_read = reads
        .iter()
        .find(|read| read.key == Key::Account(contract))
        .unwrap();
    assert_eq!(contract_read.data.operations.reads.count(), 2);
}
