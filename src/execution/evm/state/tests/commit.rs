use super::*;
use crate::execution::evm::state::StateOverlay;
use crate::execution::scalar;
use crate::namespace::Namespace;
use alloy_evm::{
    block::StateDB,
    revm::{
        Database,
        state::{Account, AccountInfo, AccountStatus, Bytecode, EvmStorageSlot},
    },
};
use alloy_primitives::{Address, U256};

fn changes(slots: &[(U256, U256, U256)]) -> EvmState {
    let mut account = Account::default();
    account.mark_touch();
    for &(key, original, present) in slots {
        account.storage.insert(
            key,
            EvmStorageSlot::new_changed(original, present, Default::default()),
        );
    }
    EvmState::from_iter([(Address::ZERO, account)])
}

#[test]
fn commit_applies_scalar_materialization_and_keeps_other_slots() {
    fn requires_state_db<T: StateDB>() {}
    requires_state_db::<StateDb>();

    let value = scalar::KeyCodec::encode(Namespace::U256, scalar::Property::Value as u8, Address::ZERO);
    let delta = scalar::KeyCodec::encode(Namespace::U256, scalar::Property::PosDelta as u8, Address::ZERO);
    let ordinary = U256::from(7);
    let mut db = StateDb::new(changes(&[
        (value, U256::ZERO, U256::from(42)),
        (delta, U256::ZERO, U256::from(5)),
        (ordinary, U256::ZERO, U256::from(99)),
    ]));
    db.commit(changes(&[
        (value, U256::from(42), U256::from(47)),
        (delta, U256::from(5), U256::ZERO),
    ]));
    assert_eq!(db.storage(Address::ZERO, value).unwrap(), U256::from(47));
    assert_eq!(db.storage(Address::ZERO, delta).unwrap(), U256::ZERO);
    assert_eq!(db.storage(Address::ZERO, ordinary).unwrap(), U256::from(99));
}

#[test]
fn commit_hides_deleted_accounts() {
    let slot = U256::from(7);
    let mut db = StateDb::new(changes(&[(slot, U256::ZERO, U256::from(99))]));
    let mut deletion = Account::default();
    deletion.mark_touch();
    deletion.mark_selfdestruct();
    db.commit(EvmState::from_iter([(Address::ZERO, deletion)]));
    assert_eq!(db.basic(Address::ZERO).unwrap(), None);
    assert_eq!(db.storage(Address::ZERO, slot).unwrap(), U256::ZERO);

    let read = Account::new_not_existing(Default::default());
    db.commit(EvmState::from_iter([(Address::ZERO, read)]));
    assert!(db.inner()[&Address::ZERO].is_selfdestructed());
    assert_eq!(db.basic(Address::ZERO).unwrap(), None);
}

#[test]
fn commit_preserves_status_across_read_only_jobs() {
    for (status, expected) in [
        (
            AccountStatus::Touched | AccountStatus::Cold,
            AccountStatus::Touched,
        ),
        (
            AccountStatus::Touched
                | AccountStatus::Created
                | AccountStatus::CreatedLocal
                | AccountStatus::Cold,
            AccountStatus::Touched | AccountStatus::Created,
        ),
    ] {
        let mut account = Account::from(AccountInfo::default().with_nonce(1));
        account.status = status;
        let mut read = Account::from(account.info.clone());
        read.mark_cold();
        let mut db = StateDb::new(EvmState::from_iter([(Address::ZERO, account)]));
        db.commit(EvmState::from_iter([(Address::ZERO, read)]));
        assert!(db.commit_error().is_none());
        assert_eq!(db.inner()[&Address::ZERO].status, expected);
    }
}

#[test]
fn commit_preserves_sequence_originals_across_transactions() {
    let slot = U256::from(7);
    let mut first = changes(&[(slot, U256::from(10), U256::from(12))]);
    let account = first.get_mut(&Address::ZERO).unwrap();
    account.original_info_mut().nonce = 3;
    account.info.nonce = 4;
    let mut db = StateDb::new(first);
    let mut second = changes(&[(slot, U256::from(12), U256::from(15))]);
    let account = second.get_mut(&Address::ZERO).unwrap();
    account.original_info_mut().nonce = 4;
    account.info.nonce = 5;
    db.commit(second);
    let account = &db.inner()[&Address::ZERO];
    assert_eq!(account.original_info().nonce, 3);
    assert_eq!(account.info.nonce, 5);
    assert_eq!(account.storage[&slot].original_value(), U256::from(10));
    assert_eq!(account.storage[&slot].present_value(), U256::from(15));
}

#[test]
fn commit_updates_code_and_keeps_it_when_not_loaded_again() {
    let code = Bytecode::new_raw(vec![0x60, 0x01, 0x00].into());
    let original = AccountInfo::default();
    let mut db = StateDb::new(EvmState::from_iter([(
        Address::ZERO,
        Account::from(original.clone()),
    )]));
    let mut incoming = Account::from(original.clone()).with_touched_mark();
    incoming.info.code_hash = code.hash_slow();
    incoming.info.code = Some(code.clone());
    db.commit(EvmState::from_iter([(Address::ZERO, incoming)]));
    assert_eq!(db.inner()[&Address::ZERO].original_info(), original);
    assert_eq!(db.code_by_hash(code.hash_slow()).unwrap(), code);

    let mut incoming = Account::from(db.inner()[&Address::ZERO].info.clone()).with_touched_mark();
    incoming.info.code = None;
    db.commit(EvmState::from_iter([(Address::ZERO, incoming)]));
    assert_eq!(db.inner()[&Address::ZERO].info.code, Some(code));
}

#[test]
fn commit_rebases_slots_on_fallback_and_keeps_their_originals() {
    let delta = scalar::KeyCodec::encode(Namespace::U256, scalar::Property::PosDelta as u8, Address::ZERO);
    let value = scalar::KeyCodec::encode(Namespace::U256, scalar::Property::Value as u8, Address::ZERO);
    let ordinary = U256::from(7);
    let fallback = StateDb::new(changes(&[
        (delta, U256::ZERO, U256::from(10)),
        (value, U256::ZERO, U256::from(42)),
        (ordinary, U256::ZERO, U256::from(50)),
    ]));
    let mut db = StateDb::with_fallback(
        StateOverlay::new(EvmState::default(), StateOverlay::add_balance_nonce_deltas),
        Some(&fallback),
    );
    db.commit(changes(&[
        (delta, U256::from(12), U256::from(15)),
        (value, U256::from(42), U256::from(47)),
        (ordinary, U256::from(55), U256::from(60)),
    ]));
    // Scalar deltas are merged by scalar::Applier, so a plain commit takes the incoming value.
    for (key, original, present) in [(delta, 10, 15), (value, 42, 47), (ordinary, 50, 60)] {
        let slot = &db.inner()[&Address::ZERO].storage[&key];
        assert_eq!(slot.original_value(), U256::from(original));
        assert_eq!(slot.present_value(), U256::from(present));
    }
    db.commit(changes(&[(delta, U256::from(15), U256::from(17))]));
    let slot = &db.inner()[&Address::ZERO].storage[&delta];
    assert_eq!(slot.original_value(), U256::from(10));
    assert_eq!(slot.present_value(), U256::from(17));
    assert_eq!(
        fallback.inner()[&Address::ZERO].storage[&delta].present_value(),
        U256::from(10)
    );
}

#[test]
fn apply_checks_deltas_for_empty_accounts_without_partial_changes() {
    let state = EvmState::from_iter([(Address::ZERO, Account::default())]);
    let mut overlay = StateOverlay::new(state.clone(), StateOverlay::add_balance_nonce_deltas);
    let mut invalid = Account::from(AccountInfo::default().with_balance(U256::from(10)));
    invalid.info.balance = U256::from(5);
    let incoming = EvmState::from_iter([
        (Address::ZERO, invalid),
        (Address::with_last_byte(1), Account::default()),
    ]);
    assert!(overlay.apply::<StateDb>(incoming, None).is_err());
    assert_eq!(overlay.inner(), &state);
}

#[test]
fn commit_preserves_first_error_and_stops_subsequent_commits() {
    let address = Address::repeat_byte(0x11);
    let initial = EvmState::from_iter([(
        address,
        Account::from(
            AccountInfo::default()
                .with_balance(U256::from(5))
                .with_nonce(1),
        ),
    )]);
    let mut db: StateDb = StateDb::with_fallback(
        StateOverlay::new(initial.clone(), StateOverlay::add_balance_nonce_deltas),
        None,
    );
    let mut invalid = Account::from(
        AccountInfo::default()
            .with_balance(U256::from(10))
            .with_nonce(1),
    );
    invalid.info.balance = U256::ZERO;
    db.commit(EvmState::from_iter([(address, invalid)]));
    let first_error = db.commit_error().unwrap().to_string();
    assert!(first_error.contains(&address.to_string()));
    assert!(first_error.contains("Baseline balance 5, nonce 1"));
    assert!(first_error.contains("Incoming balance 10 -> 0, nonce 1 -> 1"));
    assert!(first_error.contains("Underflow"));
    assert_eq!(db.inner(), &initial);

    db.commit(changes(&[(U256::ZERO, U256::ZERO, U256::from(1))]));
    assert_eq!(db.inner(), &initial);
    assert_eq!(db.commit_error().unwrap().to_string(), first_error);
}
