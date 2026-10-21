use super::*;
use alloy_evm::revm::state::{Account, EvmStorageSlot, TransactionId};

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
    assert!(cache.committer.buffered.is_empty());
}

#[test]
fn pairwise_commit_preserves_storage_resets_across_groups() {
    for deleted in [false, true] {
        let mut cache = EvmStateCache::new(state(0, &[(99, 99)]));
        for id in 1..=5_u64 {
            let mut incoming = state(id, &[(id, id)]);
            if id == 3 {
                let account = incoming.get_mut(&Address::ZERO).unwrap();
                if deleted {
                    account.mark_selfdestruct();
                } else {
                    account.mark_created();
                }
            }
            cache.buffer(id, incoming);
        }

        cache.commit();

        let account = &cache.inner.inner[&Address::ZERO];
        assert_eq!(account.info.balance, U256::from(5));
        assert_eq!(account.storage.len(), 3);
        for id in 3..=5_u64 {
            assert_eq!(
                account.storage[&U256::from(id)].present_value(),
                U256::from(id)
            );
        }
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
    assert!(cache.committer.buffered.is_empty());
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
