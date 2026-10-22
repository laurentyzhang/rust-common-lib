use super::*;
use alloy_evm::revm::state::{Account, EvmStorageSlot, TransactionId};
use alloy_primitives::{Address, U256};

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
    let mut cache = EvmStateCache::new(state(0, &[(99, 99)]));
    for id in [5, 1, 4, 2, 3] {
        cache.buffer(id, state(id, &[(0, id * 10), (id, id)]));
    }

    cache.commit();

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
    assert!(cache.buffered.is_empty());
}

#[test]
fn pairwise_commit_preserves_creation_across_groups() {
    let mut cache = EvmStateCache::new(state(0, &[(99, 99)]));
    for id in 1..=5_u64 {
        let mut incoming = state(id, &[(id, id)]);
        if id == 3 {
            incoming.get_mut(&Address::ZERO).unwrap().mark_created();
        }
        cache.buffer(id, incoming);
    }

    cache.commit();

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
        let mut cache = EvmStateCache::new(state(0, &[(7, 7), (99, 99)]));
        cache.buffer(1, state(1, &[(7, 11)]));
        let mut deletion = state(2, &[(7, 0)]);
        deletion.get_mut(&Address::ZERO).unwrap().storage.insert(
            U256::from(7),
            EvmStorageSlot::new_changed(U256::from(11), U256::ZERO, TransactionId::ZERO),
        );
        cache.buffer(2, deletion);
        cache.buffer(3, EvmState::default());
        cache.buffer(4, EvmState::default());
        cache.buffer(
            5,
            if rewrite {
                state(5, &[(7, 23)])
            } else {
                EvmState::default()
            },
        );

        cache.commit();

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
    let mut cache = EvmStateCache::new(state(0, &[(99, 99)]));
    cache.buffer(1, state(1, &[(1, 1)]));
    cache.buffer(2, state(2, &[(2, 2)]));
    let mut deletion = state(0, &[(7, 0)]);
    deletion
        .get_mut(&Address::ZERO)
        .unwrap()
        .mark_selfdestruct();
    let expected = deletion.clone();
    cache.buffer(3, deletion);
    cache.buffer(4, EvmState::default());
    cache.buffer(5, EvmState::default());

    cache.commit();

    assert_eq!(cache.into_evm_state(), expected);
}

#[test]
fn recreation_replaces_an_earlier_account_deletion() {
    let mut cache = EvmStateCache::new(state(0, &[(99, 99)]));
    let mut deletion = state(0, &[(1, 0)]);
    deletion
        .get_mut(&Address::ZERO)
        .unwrap()
        .mark_selfdestruct();
    cache.buffer(1, deletion);
    let mut creation = state(2, &[(2, 2)]);
    creation.get_mut(&Address::ZERO).unwrap().mark_created();
    cache.buffer(2, creation);
    cache.buffer(3, state(3, &[(3, 3)]));
    cache.buffer(4, state(4, &[(4, 4)]));
    cache.buffer(5, state(5, &[(5, 5)]));

    cache.commit();

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
    let mut cache = EvmStateCache::default();
    cache.buffer(1, state(1, &[(1, 1)]));
    cache.buffer(2, state(2, &[(2, 2)]));
    cache.buffer(2, state(20, &[(2, 20)]));
    cache.buffer(3, state(3, &[(3, 3)]));
    cache.stage(vec![1, 2]);

    cache.flush();

    let account = &cache.inner.inner[&Address::ZERO];
    assert_eq!(account.info.balance, U256::from(20));
    assert_eq!(account.storage.len(), 2);
    assert_eq!(
        account.storage[&U256::from(2)].present_value(),
        U256::from(20)
    );
    assert!(cache.buffered.is_empty());
}

#[test]
fn empty_commits_preserve_cache_and_later_commits_keep_previous_slots() {
    let mut cache = EvmStateCache::new(state(0, &[(99, 99)]));
    let original = cache.inner.clone();
    cache.commit();
    assert_eq!(cache.inner, original);

    cache.buffer(1, state(1, &[(1, 1)]));
    cache.commit();
    cache.buffer(2, state(2, &[(2, 2)]));
    cache.commit();
    let committed = cache.inner.clone();
    cache.flush();
    assert_eq!(cache.inner, committed);
    assert_eq!(cache.inner.inner[&Address::ZERO].storage.len(), 3);
}

#[test]
fn into_evm_state_returns_accumulated_state() {
    let mut cache = EvmStateCache::new(state(1, &[(1, 10)]));
    cache.buffer(2, state(2, &[(2, 20)]));

    cache.commit();
    assert_eq!(cache.into_evm_state(), state(2, &[(1, 10), (2, 20)]));
    assert!(EvmStateCache::default().into_evm_state().is_empty());
}
