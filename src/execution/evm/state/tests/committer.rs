use super::*;
use crate::execution::{Property, ScalarKeyCodec};
use crate::namespace::Namespace;
use alloy_evm::revm::state::{Account, AccountInfo, Bytecode, EvmStorageSlot};
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
    committer.flush();
    assert_eq!(
        committer.inner()[&Address::ZERO].storage[&key].present_value(),
        U256::from(15)
    );
    committer.flush();
    assert_eq!(
        committer.inner()[&Address::ZERO].storage[&key].present_value(),
        U256::from(15)
    );
    committer.buffer(ID::new(0, 4), changes(15, 16));
    committer.flush();
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
fn commit_rebases_account_deltas_and_preserves_the_base_across_layers() {
    let base = AccountInfo::default()
        .with_balance(U256::from(50))
        .with_nonce(5);
    let fallback = StateDb::new(EvmState::from_iter([(
        Address::ZERO,
        Account::from(base.clone()),
    )]));
    let mut sequence = EvmStateCommitter::new();
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

    let mut generation = EvmStateCommitter::new();
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

#[test]
fn commit_applies_present_code_and_ignores_absent_code_across_layers() {
    let base = AccountInfo::default()
        .with_balance(U256::from(100))
        .with_code(Bytecode::new_raw(vec![0x00].into()));
    let fallback = StateDb::new(EvmState::from_iter([(
        Address::ZERO,
        Account::from(base.clone()),
    )]));
    for code in [
        Bytecode::new_raw(vec![0x60, 0x00].into()),
        Bytecode::default(),
    ] {
        let update = base.clone().with_code(code.clone());
        let mut sequence = EvmStateCommitter::new();
        sequence.buffer(
            ID::new(0, 0),
            EvmState::from_iter([(
                Address::ZERO,
                Account::from(update.clone()).with_touched_mark(),
            )]),
        );
        sequence.buffer(
            ID::new(0, 1),
            account_change(base.clone(), U256::from(90), 1),
        );
        let committed = sequence.commit(&fallback).unwrap();
        assert_eq!(committed[&Address::ZERO].info.code, Some(code.clone()));
        assert_eq!(committed[&Address::ZERO].info.code_hash, update.code_hash);

        let mut generation = EvmStateCommitter::new();
        generation.buffer(ID::new(0, 0), committed);
        generation.buffer(
            ID::new(0, 2),
            account_change(base.clone(), U256::from(95), 0),
        );
        let committed = generation.commit(&fallback).unwrap();
        let account = &committed[&Address::ZERO];
        assert_eq!(account.info.code, Some(code));
        assert_eq!(account.info.code_hash, update.code_hash);
        assert_eq!(account.info.balance, U256::from(85));
        assert_eq!(account.info.nonce, 1);
    }
}

#[test]
fn commit_does_not_apply_out_of_range_balance_deltas() {
    for (base, old, new) in [
        (U256::MAX, U256::ZERO, U256::from(1)),
        (U256::ZERO, U256::from(2), U256::from(1)),
    ] {
        let fallback = StateDb::new(account_change(AccountInfo::default(), base, 0));
        let mut committer = EvmStateCommitter::new();
        committer.buffer(
            ID::new(0, 0),
            account_change(AccountInfo::default().with_balance(old), new, 0),
        );
        assert!(std::panic::catch_unwind(|| committer.commit(&fallback)).is_err());
    }
}

#[test]
#[should_panic]
fn commit_does_not_truncate_nonce_when_storing_the_u256_result() {
    let fallback = StateDb::new(account_change(AccountInfo::default(), U256::ZERO, u64::MAX));
    let mut committer = EvmStateCommitter::new();
    committer.buffer(
        ID::new(0, 0),
        account_change(AccountInfo::default(), U256::ZERO, 1),
    );
    let _ = committer.commit(&fallback);
}
