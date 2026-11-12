use super::*;
use crate::execution::{Property, ScalarKeyCodec};
use crate::namespace::Namespace;
use alloy_evm::revm::state::{Account, AccountInfo, EvmStorageSlot};
use alloy_primitives::{Address, U256};

#[test]
fn flush_combines_only_accepted_deltas_and_clears_the_buffer() {
    let key = ScalarKeyCodec::encode(Namespace::U256, Property::PosDelta as u8, Address::ZERO);
    let changes = |original: u64, present: u64| {
        let mut account = Account::default();
        account.mark_touch();
        account.storage.insert(
            key,
            EvmStorageSlot::new_changed(
                U256::from(original),
                U256::from(present),
                Default::default(),
            ),
        );
        EvmState::from_iter([(Address::ZERO, account)])
    };
    let mut committer = EvmStateCommitter::with_target(changes(0, 10));
    committer.buffer(ID::new(0, 2), changes(10, 13));
    committer.buffer(ID::new(0, 1), changes(10, 12));
    committer.buffer(ID::new(0, 3), changes(10, 99));
    committer.stage(vec![ID::new(0, 1), ID::new(0, 2)]);
    committer.flush().unwrap();
    assert_eq!(
        committer.inner()[&Address::ZERO].storage[&key].present_value(),
        U256::from(15)
    );
    committer.flush().unwrap();
    assert_eq!(
        committer.inner()[&Address::ZERO].storage[&key].present_value(),
        U256::from(15)
    );
    committer.buffer(ID::new(0, 4), changes(15, 16));
    committer.flush().unwrap();
    assert_eq!(
        committer.into_inner()[&Address::ZERO].storage[&key].present_value(),
        U256::from(16)
    );
}

fn account_change(original: AccountInfo, balance: U256, nonce: u64) -> EvmState {
    let mut account = Account::from(original).with_touched_mark();
    account.info.balance = balance;
    account.info.nonce = nonce;
    account.info.code = None;
    EvmState::from_iter([(Address::ZERO, account)])
}

#[test]
fn sequential_committer_uses_execution_order_instead_of_global_order() {
    let base = AccountInfo::default().with_balance(U256::from(100));
    let fallback = StateDb::new(EvmState::from_iter([(
        Address::ZERO,
        Account::from(base.clone()),
    )]));
    let mut first = ID::new(0, 9);
    first.job = 0;
    let mut second = ID::new(0, 1);
    second.job = 1;
    let mut committer = EvmStateCommitter::new_sequential();
    committer.buffer(second, account_change(base.clone(), U256::from(120), 2));
    committer.buffer(first, account_change(base, U256::from(110), 1));
    let committed = committer.commit(&fallback).unwrap();
    assert_eq!(committed[&Address::ZERO].info.balance, U256::from(120));
    assert_eq!(committed[&Address::ZERO].info.nonce, 2);
}

#[test]
fn commit_rebases_account_deltas_and_preserves_the_base_across_layers() {
    let base = AccountInfo::default()
        .with_balance(U256::from(50))
        .with_nonce(5);
    let fallback = StateDb::new(EvmState::from_iter([(
        Address::ZERO,
        Account::from(base.clone()),
    )]));
    let mut sequence = EvmStateCommitter::new_parallel();
    sequence.buffer(
        ID::new(0, 2),
        account_change(
            AccountInfo::default()
                .with_balance(U256::from(200))
                .with_nonce(9),
            U256::from(195),
            10,
        ),
    );
    sequence.buffer(
        ID::new(0, 1),
        account_change(
            AccountInfo::default()
                .with_balance(U256::from(100))
                .with_nonce(7),
            U256::from(111),
            8,
        ),
    );
    let committed = sequence.commit(&fallback).unwrap();
    assert_eq!(committed[&Address::ZERO].info.balance, U256::from(56));
    assert_eq!(committed[&Address::ZERO].info.nonce, 7);
    assert_eq!(committed[&Address::ZERO].original_info(), base);

    let mut generation = EvmStateCommitter::new_parallel();
    generation.buffer(ID::new(0, 1), committed);
    generation.buffer(
        ID::new(0, 3),
        account_change(base.clone(), U256::from(54), 6),
    );
    let committed = generation.commit(&fallback).unwrap();
    assert_eq!(committed[&Address::ZERO].info.balance, U256::from(60));
    assert_eq!(committed[&Address::ZERO].info.nonce, 8);
    assert_eq!(committed[&Address::ZERO].original_info(), base);
}
