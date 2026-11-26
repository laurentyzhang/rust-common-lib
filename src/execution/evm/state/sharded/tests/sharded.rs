use super::*;
use rayon::prelude::*;
use alloy_evm::revm::state::EvmStorageSlot;
use alloy_evm::revm::state::Bytecode;
use alloy_primitives::{B256, U256};

#[test]
fn an_absent_account_reads_as_none() {
    let state = Sharded::<4>::new();
    assert!(state.read(Address::ZERO, |account| account.is_none()));
}

#[test]
fn a_written_account_and_slot_are_read_back() {
    let state = Sharded::<4>::new();
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
    let state = Sharded::<8>::new();
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

    let merged = state.into_inner();
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
    let state = Sharded::<8>::new();
    for byte in 0..100u8 {
        state.write(Address::with_last_byte(byte), |account| {
            account.info.nonce = byte as u64
        });
    }
    let merged = state.into_inner();
    assert_eq!(merged.len(), 100);
    for byte in 0..100u8 {
        assert_eq!(merged[&Address::with_last_byte(byte)].info.nonce, byte as u64);
    }
}

#[test]
fn a_single_shard_holds_everything() {
    let state = Sharded::<1>::new();
    for byte in 0..10u8 {
        state.write(Address::with_last_byte(byte), |_| ());
    }
    assert_eq!(state.into_inner().len(), 10);
}

#[test]
fn parallel_writes_are_not_lost() {
    let state = Sharded::<16>::new();
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
    let merged = state.into_inner();
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
    use crate::execution::evm::api_store::ApiStore;
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
            Sharded::<8>::new(),
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

/// The cache over `db`: it converts Sets as they are loaded.
fn cache<DB>(db: DB) -> StateDb<DB, Sharded<8>> {
    StateDb::with_fallback(
        super::super::StateOverlay::new(
            Sharded::new_cache(),
            super::super::StateOverlay::add_balance_nonce_deltas,
        ),
        Some(db),
    )
}

/// A shared block state over `db`.
fn shared<DB>(db: DB) -> StateDb<DB, Sharded<8>> {
    StateDb::with_fallback(
        super::super::StateOverlay::new(
            Sharded::new(),
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
    use crate::execution::evm::api_store::ApiStore;

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
fn batched_inserts_match_locked_writes() {
    let mut updates = EvmState::default();
    for byte in 0..20u8 {
        let mut account = Account::default();
        account.info.nonce = byte as u64;
        for key in 0..30u64 {
            account.storage.insert(
                U256::from(key),
                EvmStorageSlot::new(U256::from(key + byte as u64), Default::default()),
            );
        }
        updates.insert(Address::with_last_byte(byte), account);
    }

    // The same updates written one by one under the locks.
    let locked = Sharded::<8>::new();
    for (&address, account) in &updates {
        locked.write(address, |current| current.info = account.info.clone());
        for (&key, slot) in &account.storage {
            locked.write_slot(address, key, |current| *current = slot.clone());
        }
    }

    // Partitioned into one batch per shard, then inserted without locks.
    let mut batched = Sharded::<8>::new();
    let batches = batched.partition(updates.clone());
    // Each batch holds only entries that belong to its shard.
    for (index, batch) in batches.iter().enumerate() {
        assert!(batch.accounts.iter().all(|&(address, _)| batched.index(address) == index));
        assert!(batch
            .slots
            .iter()
            .all(|&(address, key, _)| batched.slot_index(address, key) == index));
    }
    assert_eq!(batches.iter().map(|batch| batch.accounts.len()).sum::<usize>(), 20);
    assert_eq!(batches.iter().map(|batch| batch.slots.len()).sum::<usize>(), 600);
    batched.write_batches(batches);
    assert_eq!(
        batched.into_inner(),
        locked.into_inner()
    );
}

#[test]
fn batched_inserts_replace_info_and_keep_other_slots() {
    let address = Address::with_last_byte(1);
    let mut state = Sharded::<8>::new();
    state.write(address, |account| account.info.nonce = 1);
    for key in 0..10u64 {
        state.write_slot(address, U256::from(key), |slot| slot.present_value = U256::from(1));
    }

    let mut account = Account::default();
    account.info.nonce = 2;
    account
        .storage
        .insert(U256::from(3), EvmStorageSlot::new(U256::from(9), Default::default()));
    let batches = state.partition(EvmState::from_iter([(address, account)]));
    state.write_batches(batches);

    let merged = state.into_inner();
    assert_eq!(merged[&address].info.nonce, 2);
    assert_eq!(merged[&address].storage.len(), 10);
    assert_eq!(merged[&address].storage[&U256::from(3)].present_value, U256::from(9));
    assert_eq!(merged[&address].storage[&U256::from(4)].present_value, U256::from(1));
}

#[test]
fn a_sharded_committer_applies_crdt_deltas_like_a_plain_one() {
    use super::super::EvmStateCommitter;
    use crate::execution::scalar;
    use crate::execution::workload::ID;
    use crate::namespace::Namespace;
    use crate::store::traits::Committer;

    let owners: Vec<Address> = (1..=20u8).map(Address::with_last_byte).collect();
    let key = |owner| {
        scalar::KeyCodec::encode(Namespace::U256, scalar::Property::PosDelta as u8, owner)
    };
    let info = alloy_evm::revm::state::AccountInfo {
        balance: U256::from(100),
        ..Default::default()
    };
    // Every owner starts with a committed PosDelta total of 10.
    let base = StateDb::new(
        owners
            .iter()
            .map(|&owner| {
                let mut account = Account::from(info.clone());
                account
                    .storage
                    .insert(key(owner), EvmStorageSlot::new(U256::from(10), Default::default()));
                (owner, account)
            })
            .collect(),
    );
    // A state adding `amount` to every owner's PosDelta.
    let change = |amount: u64| -> EvmState {
        owners
            .iter()
            .map(|&owner| {
                let mut account = Account::from(info.clone());
                account.mark_touch();
                account.storage.insert(
                    key(owner),
                    EvmStorageSlot::new_changed(
                        U256::from(10),
                        U256::from(10 + amount),
                        Default::default(),
                    ),
                );
                (owner, account)
            })
            .collect()
    };
    let [first, second] = [(1, change(2)), (2, change(3))]
        .map(|(sequence, state)| (ID { sequence, ..ID::new(0, sequence) }, state));

    let mut plain = EvmStateCommitter::with_state(StateDb::with_fallback(
        super::super::StateOverlay::new(
            EvmState::default(),
            super::super::StateOverlay::add_balance_nonce_deltas,
        ),
        Some(&base),
    ));
    let mut sharded = EvmStateCommitter::with_state(shared(&base));
    for (id, state) in [first, second] {
        plain.buffer(id, state.clone());
        sharded.buffer(id, state);
    }
    let plain = plain.commit().unwrap().into_inner();
    let sharded = sharded.commit().unwrap().into_inner();

    assert_eq!(sharded, plain);
    for &owner in &owners {
        let slot = &sharded[&owner].storage[&key(owner)];
        assert_eq!((slot.original_value, slot.present_value), (U256::from(10), U256::from(15)));
        assert_eq!(sharded[&owner].info.balance, U256::from(100));
    }
}

/// A database holding `owner` with a Set stored as the list 10 → 20 → 30.
fn stored_set(owner: Address, set: [u8; 6]) -> Counting {
    use crate::execution::slot::set::{KeyCodec, Property};
    let mut account = Account::from(alloy_evm::revm::state::AccountInfo {
        balance: U256::from(1),
        ..Default::default()
    });
    let slot = |value: u64| EvmStorageSlot::new(U256::from(value), Default::default());
    account.storage.insert(KeyCodec::length(set), slot(3));
    account.storage.insert(KeyCodec::head(set), slot(10));
    account.storage.insert(KeyCodec::tail(set), slot(30));
    account
        .storage
        .insert(KeyCodec::element(Property::Node, set, U256::from(10)), slot(20));
    account
        .storage
        .insert(KeyCodec::element(Property::Node, set, U256::from(20)), slot(30));
    Counting {
        db: StateDb::new(EvmState::from_iter([(owner, account)])),
        ..Default::default()
    }
}

#[test]
fn the_cache_converts_a_set_on_its_first_load() {
    use crate::execution::slot::set::{KeyCodec, Property};
    let owner = Address::with_last_byte(1);
    let set = [1u8; 6];
    let db = stored_set(owner, set);
    let state = cache(&db);
    state.basic_ref(owner).unwrap();

    // Reading any of the Set's slots converts it in the shards.
    assert_eq!(state.storage_ref(owner, KeyCodec::indexed(set)).unwrap(), U256::from(1));
    let index = KeyCodec::delta(Property::Index, set);
    assert_eq!(state.storage_ref(owner, index.key(0).unwrap()).unwrap(), U256::from(3 * 32));
    for (at, element) in [10u64, 20, 30].into_iter().enumerate() {
        assert_eq!(
            state.storage_ref(owner, index.key(at as u64 + 1).unwrap()).unwrap(),
            U256::from(element)
        );
        let position = KeyCodec::element(Property::Position, set, U256::from(element));
        assert_eq!(state.storage_ref(owner, position).unwrap(), U256::from(at + 1));
    }

    // Later reads find it converted and load nothing more.
    let loads = db.slots.load(Relaxed);
    assert_eq!(state.storage_ref(owner, KeyCodec::indexed(set)).unwrap(), U256::from(1));
    assert_eq!(db.slots.load(Relaxed), loads);
}

#[test]
fn a_sequence_over_the_block_state_finds_the_set_converted_by_the_cache() {
    use crate::execution::slot::set::KeyCodec;
    let owner = Address::with_last_byte(1);
    let set = [1u8; 6];
    let db = stored_set(owner, set);
    let cache = cache(&db);
    cache.basic_ref(owner).unwrap();
    // The block state does not convert; it reads the Set converted from the cache.
    let state = shared(&cache);
    state.basic_ref(owner).unwrap();

    // The sequence's overlay reads through the block state and the cache; it keeps nothing of
    // the conversion.
    let sequence = StateDb::with_fallback(
        super::super::StateOverlay::new(
            EvmState::default(),
            super::super::StateOverlay::copy_balance_nonce,
        ),
        Some(&state),
    );
    assert_eq!(sequence.storage_ref(owner, KeyCodec::indexed(set)).unwrap(), U256::from(1));
    assert!(sequence.into_inner().is_empty());
}

#[test]
fn a_set_is_not_converted_before_its_account_is_loaded() {
    use crate::execution::slot::set::KeyCodec;
    let owner = Address::with_last_byte(1);
    let set = [1u8; 6];
    let db = stored_set(owner, set);
    let state = cache(&db);
    assert_eq!(state.storage_ref(owner, KeyCodec::indexed(set)).unwrap(), U256::ZERO);
    state.basic_ref(owner).unwrap();
    assert_eq!(state.storage_ref(owner, KeyCodec::indexed(set)).unwrap(), U256::from(1));
}

#[test]
fn a_plain_state_does_not_convert_sets() {
    use crate::execution::slot::set::KeyCodec;
    let owner = Address::with_last_byte(1);
    let set = [1u8; 6];
    let db = stored_set(owner, set);
    let state = StateDb::with_fallback(
        super::super::StateOverlay::new(
            EvmState::default(),
            super::super::StateOverlay::copy_balance_nonce,
        ),
        Some(&db),
    );
    state.basic_ref(owner).unwrap();
    assert_eq!(state.storage_ref(owner, KeyCodec::indexed(set)).unwrap(), U256::ZERO);
}

#[test]
fn a_block_state_does_not_convert_sets() {
    use crate::execution::slot::set::KeyCodec;
    let owner = Address::with_last_byte(1);
    let set = [1u8; 6];
    let db = stored_set(owner, set);
    let state = shared(&db);
    state.basic_ref(owner).unwrap();
    assert_eq!(state.storage_ref(owner, KeyCodec::indexed(set)).unwrap(), U256::ZERO);
}
