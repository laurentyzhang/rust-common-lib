use super::*;
use alloy_evm::revm::state::EvmStorageSlot;
use alloy_evm::revm::state::Bytecode;
use alloy_primitives::{B256, U256};
use rayon::prelude::*;

#[test]
fn an_absent_account_reads_as_none() {
    let state = Shared::<4>::new();
    assert!(state.read(Address::ZERO, |account| account.is_none()));
}

#[test]
fn a_written_account_and_slot_are_read_back() {
    let state = Shared::<4>::new();
    let address = Address::with_last_byte(1);
    state.write(address, |account| account.info.balance = U256::from(7));
    state.write(address, |account| account.info.nonce = 3);
    state.write_slot(address, U256::from(1), |slot| slot.present_value = U256::from(9));

    state.read(address, |account| {
        let account = account.unwrap();
        assert_eq!(account.info.balance, U256::from(7));
        assert_eq!(account.info.nonce, 3);
    });
    assert_eq!(
        state.read_slot(address, U256::from(1), |slot| slot.unwrap().present_value),
        U256::from(9)
    );
    assert!(state.read_slot(address, U256::from(2), |slot| slot.is_none()));
    assert!(state.read(Address::with_last_byte(2), |account| account.is_none()));
}

#[test]
fn one_accounts_slots_spread_across_shards_and_merge_back() {
    let state = Shared::<8>::new();
    let address = Address::with_last_byte(1);
    state.write(address, |account| account.info.nonce = 5);
    for key in 0..100u64 {
        state.write_slot(address, U256::from(key), |slot| {
            *slot = EvmStorageSlot::new(U256::from(key + 1), Default::default())
        });
    }
    // The account appears in more than one shard.
    let holding = state
        .shards
        .iter()
        .filter(|shard| shard.read().unwrap().contains_key(&address))
        .count();
    assert!(holding > 1);

    let merged = state.into_unlocked().into_inner();
    assert_eq!(merged.len(), 1);
    let account = &merged[&address];
    assert_eq!(account.info.nonce, 5);
    assert_eq!(account.storage.len(), 100);
    for key in 0..100u64 {
        assert_eq!(account.storage[&U256::from(key)].present_value, U256::from(key + 1));
    }
}

#[test]
fn into_inner_collects_every_shard() {
    let state = Shared::<8>::new();
    for byte in 0..100u8 {
        state.write(Address::with_last_byte(byte), |account| {
            account.info.nonce = byte as u64
        });
    }
    let merged = state.into_unlocked().into_inner();
    assert_eq!(merged.len(), 100);
    for byte in 0..100u8 {
        assert_eq!(merged[&Address::with_last_byte(byte)].info.nonce, byte as u64);
    }
}

#[test]
fn a_single_shard_holds_everything() {
    let state = Shared::<1>::new();
    for byte in 0..10u8 {
        state.write(Address::with_last_byte(byte), |_| ());
    }
    assert_eq!(state.into_unlocked().into_inner().len(), 10);
}

#[test]
fn parallel_writes_are_not_lost() {
    let state = Shared::<16>::new();
    let shared = Address::repeat_byte(0xff);
    // Many threads write distinct accounts and slots, and all add to one shared account and slot.
    (0..1000u64).into_par_iter().for_each(|i| {
        state.write(Address::left_padding_from(&i.to_be_bytes()), |account| {
            account.info.nonce = i
        });
        state.write(shared, |account| account.info.balance += U256::from(1));
        state.write_slot(shared, U256::from(i), |slot| slot.present_value = U256::from(i));
        state.write_slot(shared, U256::MAX, |slot| slot.present_value += U256::from(1));
    });
    assert_eq!(
        state.read(shared, |account| account.unwrap().info.balance),
        U256::from(1000)
    );
    let merged = state.into_unlocked().into_inner();
    assert_eq!(merged.len(), 1001);
    for i in 0..1000u64 {
        assert_eq!(merged[&Address::left_padding_from(&i.to_be_bytes())].info.nonce, i);
        assert_eq!(merged[&shared].storage[&U256::from(i)].present_value, U256::from(i));
    }
    assert_eq!(merged[&shared].info.balance, U256::from(1000));
    assert_eq!(merged[&shared].storage[&U256::MAX].present_value, U256::from(1000));
}

#[test]
fn a_sharded_state_db_matches_a_plain_one() {
    use super::super::{StateDb, overlay::StateOverlay};
    use crate::execution::evm::storage::ApiStore;
    use alloy_evm::revm::{Database, DatabaseCommit, state::AccountInfo};

    let address = Address::with_last_byte(1);
    let base = StateDb::new(EvmState::from_iter([(
        address,
        Account::from(AccountInfo {
            balance: U256::from(100),
            nonce: 3,
            ..Default::default()
        }),
    )]));
    // One change touching the balance and many slots of the same account.
    let mut account = Account::from(AccountInfo {
        balance: U256::from(90),
        nonce: 4,
        ..Default::default()
    });
    *account.original_info_mut() = AccountInfo {
        balance: U256::from(100),
        nonce: 3,
        ..Default::default()
    };
    account.mark_touch();
    for key in 0..50u64 {
        account.storage.insert(
            U256::from(key),
            EvmStorageSlot::new_changed(U256::ZERO, U256::from(key + 1), Default::default()),
        );
    }
    let change = EvmState::from_iter([(address, account)]);

    let mut plain = StateDb::with_fallback(
        StateOverlay::new(EvmState::default(), StateOverlay::add_balance_nonce_deltas),
        Some(&base),
    );
    let mut sharded = StateDb::with_fallback(
        StateOverlay::new(
            Shared::<8>::new(),
            StateOverlay::add_balance_nonce_deltas,
        ),
        Some(&base),
    );
    for db in [&mut plain as &mut dyn DatabaseCommit, &mut sharded] {
        db.commit(change.clone());
    }
    plain.write(address, U256::from(100), U256::from(7)).unwrap();
    sharded.write(address, U256::from(100), U256::from(7)).unwrap();

    assert_eq!(sharded.basic(address).unwrap(), plain.basic(address).unwrap());
    for key in [0u64, 25, 49, 100, 200] {
        assert_eq!(
            sharded.storage(address, U256::from(key)).unwrap(),
            plain.storage(address, U256::from(key)).unwrap()
        );
    }
    // The sharded state also caches the slots it read; the changes are the same.
    let changes = |mut state: EvmState| {
        for account in state.values_mut() {
            account.storage.retain(|_, slot| slot.is_changed());
        }
        state
    };
    assert_eq!(changes(sharded.into_inner()), changes(plain.into_inner()));
}

/// Counts the reads that reach the wrapped database.
#[derive(Debug, Default)]
struct Counting {
    db: StateDb,
    accounts: std::sync::atomic::AtomicUsize,
    slots: std::sync::atomic::AtomicUsize,
}

impl alloy_evm::revm::DatabaseRef for Counting {
    type Error = std::convert::Infallible;

    /// Counts and forwards an account read.
    fn basic_ref(
        &self,
        address: Address,
    ) -> Result<Option<alloy_evm::revm::state::AccountInfo>, Self::Error> {
        self.accounts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.db.basic_ref(address)
    }

    /// Forwards a code read.
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.db.code_by_hash_ref(code_hash)
    }

    /// Counts and forwards a slot read.
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.slots.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.db.storage_ref(address, index)
    }

    /// Forwards a block hash read.
    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.db.block_hash_ref(number)
    }
}

use super::super::StateDb;
use alloy_evm::revm::DatabaseRef;
use std::sync::atomic::Ordering::Relaxed;

/// A database holding `address` with balance 5 and slot 1 = 11.
fn counting(address: Address) -> Counting {
    let mut account = Account::from(alloy_evm::revm::state::AccountInfo {
        balance: U256::from(5),
        ..Default::default()
    });
    account
        .storage
        .insert(U256::from(1), EvmStorageSlot::new(U256::from(11), Default::default()));
    Counting {
        db: StateDb::new(EvmState::from_iter([(address, account)])),
        ..Default::default()
    }
}

/// A shared block state over `db`.
fn shared<DB>(db: DB) -> StateDb<DB, Shared<8>> {
    StateDb::with_fallback(
        super::super::StateOverlay::new(
            Shared::new(),
            super::super::StateOverlay::add_balance_nonce_deltas,
        ),
        Some(db),
    )
}

#[test]
fn a_shared_state_loads_each_account_and_slot_once() {
    let address = Address::with_last_byte(1);
    let db = counting(address);
    let state = shared(&db);
    for _ in 0..3 {
        assert_eq!(state.basic_ref(address).unwrap().unwrap().balance, U256::from(5));
        assert_eq!(state.storage_ref(address, U256::from(1)).unwrap(), U256::from(11));
        assert_eq!(state.storage_ref(address, U256::from(2)).unwrap(), U256::ZERO);
    }
    assert_eq!(db.accounts.load(Relaxed), 1);
    assert_eq!(db.slots.load(Relaxed), 2);

    // Cached values are unchanged: original equals present.
    let merged = state.into_inner();
    assert!(merged[&address].storage.values().all(|slot| !slot.is_changed()));
    assert!(!merged[&address].is_touched());
}

#[test]
fn a_plain_state_does_not_cache() {
    let address = Address::with_last_byte(1);
    let db = counting(address);
    let state = StateDb::with_fallback(
        super::super::StateOverlay::new(
            EvmState::default(),
            super::super::StateOverlay::copy_balance_nonce,
        ),
        Some(&db),
    );
    for _ in 0..3 {
        state.basic_ref(address).unwrap();
        state.storage_ref(address, U256::from(1)).unwrap();
    }
    assert_eq!(db.accounts.load(Relaxed), 3);
    assert_eq!(db.slots.load(Relaxed), 3);
    assert!(state.into_inner().is_empty());
}

#[test]
fn a_cached_read_does_not_replace_a_write() {
    use crate::execution::evm::storage::ApiStore;

    let address = Address::with_last_byte(1);
    let db = counting(address);
    let mut state = shared(&db);
    // Read (and cache) the old value, write a new one, then read again.
    assert_eq!(state.storage_ref(address, U256::from(1)).unwrap(), U256::from(11));
    state.write(address, U256::from(1), U256::from(12)).unwrap();
    assert_eq!(state.storage_ref(address, U256::from(1)).unwrap(), U256::from(12));
    // The account keeps its loaded balance.
    assert_eq!(state.basic_ref(address).unwrap().unwrap().balance, U256::from(5));
    let merged = state.into_inner();
    let slot = &merged[&address].storage[&U256::from(1)];
    assert_eq!((slot.original_value, slot.present_value), (U256::from(11), U256::from(12)));
}

#[test]
fn a_slot_read_for_a_missing_account_does_not_create_it() {
    let missing = Address::with_last_byte(9);
    let db = counting(Address::with_last_byte(1));
    let state = shared(&db);
    // Read many slots so some land in the account's own shard.
    for key in 0..64u64 {
        assert_eq!(state.storage_ref(missing, U256::from(key)).unwrap(), U256::ZERO);
    }
    assert_eq!(state.basic_ref(missing).unwrap(), None);
    assert_eq!(state.basic_ref(missing).unwrap(), None);
    // Missing accounts are not cached.
    assert_eq!(db.accounts.load(Relaxed), 2);
}

#[test]
fn parallel_readers_share_the_loaded_values() {
    let address = Address::with_last_byte(1);
    let db = counting(address);
    let state = shared(&db);
    // As in the EVM, the account is loaded before its slots.
    state.basic_ref(address).unwrap();
    (0..1000u64).into_par_iter().for_each(|i| {
        assert_eq!(state.storage_ref(address, U256::from(1)).unwrap(), U256::from(11));
        assert_eq!(state.storage_ref(address, U256::from(i % 10 + 2)).unwrap(), U256::ZERO);
    });
    // Racing misses may each load, but every key ends up cached once with the loaded value.
    let merged = state.into_inner();
    assert_eq!(merged[&address].storage.len(), 11);
    assert_eq!(merged[&address].storage[&U256::from(1)].present_value, U256::from(11));
}

#[test]
fn lock_free_shards_match_the_locked_ones() {
    let address = Address::with_last_byte(1);
    let mut plain = Sharded::<EvmState, 8>::new();
    plain.account_mut(address).info.nonce = 5;
    for key in 0..50u64 {
        plain.slot_mut(address, U256::from(key)).present_value = U256::from(key + 1);
    }
    assert_eq!(plain.account(address).unwrap().info.nonce, 5);
    assert_eq!(plain.slot(address, U256::from(7)).unwrap().present_value, U256::from(8));
    assert!(plain.slot(address, U256::from(99)).is_none());

    let locked = Shared::<8>::new();
    locked.write(address, |account| account.info.nonce = 5);
    for key in 0..50u64 {
        locked.write_slot(address, U256::from(key), |slot| slot.present_value = U256::from(key + 1));
    }
    // Removing the locks keeps every entry in place.
    let unlocked = locked.into_unlocked();
    assert_eq!(unlocked.slot(address, U256::from(7)).unwrap().present_value, U256::from(8));
    assert_eq!(unlocked.into_inner(), plain.into_inner());
}
