use super::*;
use crate::eu::{ApiAddress, Field, KeyCodec};
use alloy_evm::{
    block::StateDB,
    revm::{
        Database,
        state::{Account, AccountInfo, EvmStorageSlot},
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

    let value = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let delta = KeyCodec::encode(ApiAddress::Scalar, Field::PosDelta as u8, Address::ZERO);
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
fn commit_hides_deleted_accounts_and_recreation_resets_storage() {
    let slot = U256::from(7);
    let mut db = StateDb::new(changes(&[(slot, U256::ZERO, U256::from(99))]));
    let mut deletion = Account::default();
    deletion.mark_touch();
    deletion.mark_selfdestruct();
    db.commit(EvmState::from_iter([(Address::ZERO, deletion)]));
    assert_eq!(db.basic(Address::ZERO).unwrap(), None);
    assert_eq!(db.storage(Address::ZERO, slot).unwrap(), U256::ZERO);

    let info = AccountInfo {
        nonce: 1,
        ..Default::default()
    };
    let mut recreated = Account::from(info.clone());
    recreated.mark_touch();
    recreated.mark_created();
    db.commit(EvmState::from_iter([(Address::ZERO, recreated)]));
    assert_eq!(db.basic(Address::ZERO).unwrap(), Some(info));
    assert_eq!(db.storage(Address::ZERO, slot).unwrap(), U256::ZERO);
}
