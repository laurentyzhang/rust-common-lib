use super::{EvmStateCache, EvmStateCommitter};
use crate::execution::evm::{
    ArcologyEvmFactory, ArcologyTx, CallHookRouter, arcology_address, inspector::StateReadRecorder,
    key::Key,
};
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
    let mut committer = EvmStateCommitter::new(EvmStateCache::new(state(0, &[(99, 99)])));
    for id in [5, 1, 4, 2, 3] {
        committer.buffer(id, state(id, &[(0, id * 10), (id, id)]));
    }

    committer.flush();
    let cache = committer.into_inner();

    let account = &cache.inner.inner[&Address::ZERO];
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
    let mut committer = EvmStateCommitter::new(EvmStateCache::new(state(0, &[(99, 99)])));
    for id in 1..=5_u64 {
        let mut incoming = state(id, &[(id, id)]);
        if id == 3 {
            incoming.get_mut(&Address::ZERO).unwrap().mark_created();
        }
        committer.buffer(id, incoming);
    }

    committer.flush();
    let cache = committer.into_inner();

    let account = &cache.inner.inner[&Address::ZERO];
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
        let mut committer =
            EvmStateCommitter::new(EvmStateCache::new(state(0, &[(7, 7), (99, 99)])));
        committer.buffer(1, state(1, &[(7, 11)]));
        let mut deletion = state(2, &[(7, 0)]);
        deletion.get_mut(&Address::ZERO).unwrap().storage.insert(
            U256::from(7),
            EvmStorageSlot::new_changed(U256::from(11), U256::ZERO, TransactionId::ZERO),
        );
        committer.buffer(2, deletion);
        committer.buffer(3, EvmState::default());
        committer.buffer(4, EvmState::default());
        committer.buffer(
            5,
            if rewrite {
                state(5, &[(7, 23)])
            } else {
                EvmState::default()
            },
        );

        committer.flush();
        let cache = committer.into_inner();

        let account = &cache.inner.inner[&Address::ZERO];
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
    let mut committer = EvmStateCommitter::new(EvmStateCache::new(state(0, &[(99, 99)])));
    committer.buffer(1, state(1, &[(1, 1)]));
    committer.buffer(2, state(2, &[(2, 2)]));
    let mut deletion = state(0, &[(7, 0)]);
    deletion
        .get_mut(&Address::ZERO)
        .unwrap()
        .mark_selfdestruct();
    let expected = deletion.clone();
    committer.buffer(3, deletion);
    committer.buffer(4, EvmState::default());
    committer.buffer(5, EvmState::default());

    committer.flush();
    let cache = committer.into_inner();

    assert_eq!(cache.into_evm_state(), expected);
}

#[test]
fn recreation_replaces_an_earlier_account_deletion() {
    let mut committer = EvmStateCommitter::new(EvmStateCache::new(state(0, &[(99, 99)])));
    let mut deletion = state(0, &[(1, 0)]);
    deletion
        .get_mut(&Address::ZERO)
        .unwrap()
        .mark_selfdestruct();
    committer.buffer(1, deletion);
    let mut creation = state(2, &[(2, 2)]);
    creation.get_mut(&Address::ZERO).unwrap().mark_created();
    committer.buffer(2, creation);
    committer.buffer(3, state(3, &[(3, 3)]));
    committer.buffer(4, state(4, &[(4, 4)]));
    committer.buffer(5, state(5, &[(5, 5)]));

    committer.flush();
    let cache = committer.into_inner();

    let account = &cache.inner.inner[&Address::ZERO];
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
    let mut committer = EvmStateCommitter::default();
    committer.buffer(1, state(1, &[(1, 1)]));
    committer.buffer(2, state(2, &[(2, 2)]));
    committer.buffer(2, state(20, &[(2, 20)]));
    committer.buffer(3, state(3, &[(3, 3)]));
    committer.stage(vec![1, 2]);

    committer.flush();
    let cache = committer.into_inner();

    let account = &cache.inner.inner[&Address::ZERO];
    assert_eq!(account.info.balance, U256::from(20));
    assert_eq!(account.storage.len(), 2);
    assert_eq!(
        account.storage[&U256::from(2)].present_value(),
        U256::from(20)
    );
}

#[test]
fn empty_commits_preserve_cache_and_later_commits_keep_previous_slots() {
    let cache = EvmStateCache::new(state(0, &[(99, 99)]));
    let original = cache.inner.clone();
    let mut committer = EvmStateCommitter::new(cache);
    committer.flush();
    let cache = committer.into_inner();
    assert_eq!(cache.inner, original);

    let mut committer = EvmStateCommitter::new(cache);
    committer.buffer(1, state(1, &[(1, 1)]));
    committer.flush();
    // A later batch with a lower ID must not replay states from the first batch.
    committer.buffer(0, state(2, &[(2, 2)]));
    committer.flush();
    let cache = committer.into_inner();
    let committed = cache.inner.clone();
    assert_eq!(
        cache.get(&Address::ZERO).unwrap().info.balance,
        U256::from(2)
    );
    let mut committer = EvmStateCommitter::new(cache);
    committer.flush();
    let cache = committer.into_inner();
    assert_eq!(cache.inner, committed);
    assert_eq!(cache.inner.inner[&Address::ZERO].storage.len(), 3);
}

#[test]
fn into_evm_state_returns_accumulated_state() {
    let mut committer = EvmStateCommitter::new(EvmStateCache::new(state(1, &[(1, 10)])));
    committer.buffer(2, state(2, &[(2, 20)]));

    committer.flush();
    let cache = committer.into_inner();
    assert_eq!(cache.into_evm_state(), state(2, &[(1, 10), (2, 20)]));
    assert!(EvmStateCache::default().into_evm_state().is_empty());
}

#[test]
fn into_inner_excludes_rejected_and_unflushed_states() {
    let initial = state(1, &[(1, 10)]);
    let mut committer = EvmStateCommitter::new(EvmStateCache::new(initial.clone()));
    committer.buffer(2, state(2, &[(2, 20)]));
    committer.stage(Vec::new());
    committer.flush();
    committer.buffer(3, state(3, &[(3, 30)]));

    assert_eq!(committer.into_inner().into_evm_state(), initial);
}

#[test]
fn commit_writes_account_and_storage_to_evm_state_cache() {
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

    let mut committer = EvmStateCommitter::default();
    committer.buffer(9, changes);
    committer.flush();
    let mut db = committer.into_inner();

    assert_eq!(db.basic(address).unwrap(), Some(info));
    assert_eq!(db.storage(address, slot).unwrap(), storage_value);
    assert_eq!(db.into_evm_state().len(), 1);
}

#[test]
fn evm_state_cache_and_recorder_track_balance_reads() {
    let caller = arcology_address(0x11);
    let contract = arcology_address(0x22);
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

    let factory = ArcologyEvmFactory::try_new(CallHookRouter::new()).unwrap();
    let recorder = StateReadRecorder::new(2);
    let mut evm =
        factory.create_evm_with_inspector(EvmStateCache::new(state), EvmEnv::default(), recorder);
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
