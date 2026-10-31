use super::*;
use crate::eu::{ApiAddress, Field, KeyCodec};
use alloy_evm::revm::state::{Account, EvmStorageSlot};
use alloy_primitives::{Address, U256};

#[test]
fn flush_combines_only_accepted_deltas_and_clears_the_buffer() {
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::PosDelta as u8, Address::ZERO);
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
    let mut committer = StatDbCommitter::with_state(changes(0, 10));
    committer.buffer(2, changes(10, 13));
    committer.buffer(1, changes(10, 12));
    committer.buffer(3, changes(10, 99));
    committer.stage(vec![1, 2]);
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
    committer.buffer(4, changes(15, 16));
    committer.flush();
    assert_eq!(
        committer.into_inner()[&Address::ZERO].storage[&key].present_value(),
        U256::from(16)
    );
}
