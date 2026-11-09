use super::*;
use crate::execution::evm::storage::{AccountStorage, ApiStore};
use alloy_evm::revm::{
    DatabaseCommit,
    database_interface::EmptyDB,
    state::{Account, AccountStatus, EvmState, EvmStorageSlot},
};

#[test]
fn reads_fallback_and_keeps_writes_local() {
    let owner = Address::repeat_byte(0x11);
    let code = Bytecode::new_raw(vec![0x00].into());
    let info = AccountInfo {
        balance: U256::from(50),
        code_hash: code.hash_slow(),
        code: Some(code.clone()),
        ..Default::default()
    };
    let mut account = Account::from(info.clone());
    for (key, value) in [(7, 70), (8, 80)] {
        account.storage.insert(
            U256::from(key),
            EvmStorageSlot::new(U256::from(value), Default::default()),
        );
    }
    let fallback = StateDb::new(EvmState::from_iter([(owner, account)]));
    let mut db = StateDb::with_fallback(EvmState::default(), Some(&fallback));
    assert_eq!(db.get(&owner).unwrap(), Some(info.clone()));
    assert_eq!(db.basic(owner).unwrap(), Some(info.clone()));
    assert_eq!(db.code_by_hash(info.code_hash).unwrap(), code);
    assert_eq!(db.read(owner, U256::from(7)).unwrap(), U256::from(70));
    assert!(db.inner().is_empty());

    db.write(owner, U256::from(7), U256::from(75)).unwrap();
    assert_eq!(db.inner()[&owner].info.balance, info.balance);
    assert_eq!(
        db.inner()[&owner].storage[&U256::from(7)].original_value(),
        U256::from(70)
    );
    let mut changes = Account::from(info);
    changes.info.balance = U256::from(99);
    changes.mark_touch();
    changes.storage.insert(
        U256::from(7),
        EvmStorageSlot::new_changed(U256::from(75), U256::ZERO, Default::default()),
    );
    db.commit(EvmState::from_iter([(owner, changes)]));
    assert_eq!(db.basic(owner).unwrap().unwrap().balance, U256::from(99));
    assert_eq!(db.get(&owner).unwrap().unwrap().balance, U256::from(99));
    assert_eq!(db.read(owner, U256::from(7)).unwrap(), U256::ZERO);
    assert_eq!(db.read(owner, U256::from(8)).unwrap(), U256::from(80));
    assert_eq!(
        fallback.basic_ref(owner).unwrap().unwrap().balance,
        U256::from(50)
    );
    assert_eq!(
        fallback.storage_ref(owner, U256::from(7)).unwrap(),
        U256::from(70)
    );
    assert_eq!(db.into_inner()[&owner].storage.len(), 1);
}

#[test]
fn local_account_resets_hide_fallback_storage() {
    let owner = Address::repeat_byte(0x11);
    let mut account = Account::from(AccountInfo::default());
    account.storage.insert(
        U256::from(7),
        EvmStorageSlot::new(U256::from(70), Default::default()),
    );
    let fallback = StateDb::new(EvmState::from_iter([(owner, account)]));
    let mut deleted = Account::from(AccountInfo::default());
    deleted.mark_selfdestruct();
    let mut created = Account::from(AccountInfo::default());
    created.mark_created();
    let mut missing = Account::from(AccountInfo::default());
    missing.status.insert(AccountStatus::LoadedAsNotExisting);
    for (account, exists) in [(deleted, false), (created, true), (missing, false)] {
        let mut db =
            StateDb::with_fallback(EvmState::from_iter([(owner, account)]), Some(&fallback));
        assert_eq!(db.basic(owner).unwrap().is_some(), exists);
        assert_eq!(db.get(&owner).unwrap().is_some(), exists);
        assert_eq!(db.read(owner, U256::from(7)).unwrap(), U256::ZERO);
    }
}

#[test]
fn optional_fallback_controls_block_hash_reads() {
    let mut local = StateDb::new(EvmState::default());
    assert_eq!(local.basic(Address::ZERO).unwrap(), None);
    assert_eq!(local.get(&Address::ZERO).unwrap(), None);
    assert_eq!(
        local.storage(Address::ZERO, U256::ZERO).unwrap(),
        U256::ZERO
    );
    assert_eq!(local.block_hash(42).unwrap(), B256::ZERO);
    let fallback = EmptyDB::default();
    let mut db = StateDb::with_fallback(EvmState::default(), Some(&fallback));
    assert_eq!(
        db.block_hash(42).unwrap(),
        fallback.block_hash_ref(42).unwrap()
    );
}

#[test]
fn merging_local_state_preserves_the_destination_fallback() {
    let owner = Address::repeat_byte(0x11);
    let info = AccountInfo {
        balance: U256::from(50),
        ..Default::default()
    };
    let fallback = StateDb::new(EvmState::from_iter([(owner, Account::from(info.clone()))]));
    let mut db = StateDb::with_fallback(EvmState::default(), Some(&fallback));
    db.overlay
        .merge(EvmState::default(), db.fallback.as_ref())
        .unwrap();
    assert_eq!(db.basic(owner).unwrap(), Some(info));
}
