use super::super::{EvmStateCommitter, StateDb};
use super::AccountStorage;
use crate::execution::{ArcologyEvmFactory, ArcologyTx, CallRouter, ReadRecorder, key::Key};
use crate::planner::scheduler::ID;
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

fn change(original_balance: u64, balance: u64, slots: &[(u64, u64)]) -> EvmState {
    let mut state = state(balance, slots);
    state
        .get_mut(&Address::ZERO)
        .unwrap()
        .original_info_mut()
        .balance = U256::from(original_balance);
    state
}

#[test]
fn empty_commits_preserve_cache_and_later_commits_keep_previous_slots() {
    let cache = state(0, &[(99, 99)]);
    let original = cache.clone();
    let mut accumulator = EvmStateCommitter::with_target(cache);
    accumulator.flush();
    let cache = accumulator.into_inner();
    assert_eq!(cache, original);

    let mut accumulator = EvmStateCommitter::with_target(cache);
    accumulator.buffer(ID::new(0, 1), change(0, 1, &[(1, 1)]));
    accumulator.flush();
    // A later batch with a lower ID must not replay states from the first batch.
    accumulator.buffer(ID::new(0, 0), change(1, 2, &[(2, 2)]));
    accumulator.flush();
    let cache = accumulator.into_inner();
    let committed = cache.clone();
    assert_eq!(
        cache.get(&Address::ZERO).unwrap().info.balance,
        U256::from(2)
    );
    let mut accumulator = EvmStateCommitter::with_target(cache);
    accumulator.flush();
    let cache = accumulator.into_inner();
    assert_eq!(cache, committed);
    assert_eq!(cache[&Address::ZERO].storage.len(), 3);
}

#[test]
fn into_evm_state_returns_accumulated_state() {
    let mut accumulator = EvmStateCommitter::with_target(state(1, &[(1, 10)]));
    accumulator.buffer(ID::new(0, 2), change(1, 2, &[(2, 20)]));

    accumulator.flush();
    let cache = accumulator.into_inner();
    assert_eq!(cache, state(2, &[(1, 10), (2, 20)]));
    assert!(EvmState::default().is_empty());
}

#[test]
fn into_inner_excludes_rejected_and_unflushed_states() {
    let initial = state(1, &[(1, 10)]);
    let mut accumulator = EvmStateCommitter::with_target(initial.clone());
    accumulator.buffer(ID::new(0, 2), state(2, &[(2, 20)]));
    accumulator.stage(Vec::new());
    accumulator.flush();
    accumulator.buffer(ID::new(0, 3), state(3, &[(3, 30)]));

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

    let mut accumulator = EvmStateCommitter::default();
    accumulator.buffer(ID::new(0, 9), changes);
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
