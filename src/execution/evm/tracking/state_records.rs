use alloy_evm::revm::context_interface::result::ResultAndState;

use crate::crdt::state::{
    Tracked,
    op::Operations,
    status::Tag::{self, Stripped},
    value::ValueSnapshots,
};
use crate::execution::evm::key::Key;

/// Exports REVM account and storage accesses without consuming the result.
pub fn delta_records<H>(
    result_and_state: &ResultAndState<H>,
    id: u64,
) -> Vec<(Key, Tracked<Tag, Tag>)> {
    let mut records = account_delta_records(result_and_state, id);
    records.extend(slot_delta_records(result_and_state, id));
    records
}

/// Collects account records from the result state.
fn account_delta_records<H>(
    result_and_state: &ResultAndState<H>,
    id: u64,
) -> Vec<(Key, Tracked<Tag, Tag>)> {
    let mut records = Vec::with_capacity(result_and_state.state.len());

    for (address, account) in &result_and_state.state {
        let mut operations = Operations::default();

        let original = if account.is_loaded_as_not_existing() {
            Tag::Missing
        } else {
            Tag::Stripped
        };

        let mut current = Stripped;
        if account.is_selfdestructed() {
            operations.removes.increment();
            current = Tag::Deleted;
        }

        // An account with empty storage is considered default.
        // This is not strictly necessary, since an empty
        // account will be removed anyway.
        if account.storage.is_empty() {
            operations.deltas.increment();
            current = Tag::Default
        }

        records.push((
            Key::Account(*address),
            Tracked {
                id,
                value: ValueSnapshots { original, current },
                operations,
            },
        ));
    }

    records
}

/// Collects storage records from the result state.
fn slot_delta_records<H>(
    result_and_state: &ResultAndState<H>,
    id: u64,
) -> Vec<(Key, Tracked<Tag, Tag>)> {
    let mut records = Vec::new();

    for (address, account) in &result_and_state.state {
        for (slot, value) in &account.storage {
            let mut operations = Operations::default();

            let original = if value.original_value().is_zero() {
                Tag::Missing
            } else {
                Tag::Stripped
            };

            let current = if value.present_value().is_zero() {
                operations.removes.increment();
                Tag::Deleted
            } else {
                operations.deltas.increment();
                Tag::Stripped
            };

            records.push((
                Key::Storage(*address, *slot),
                Tracked {
                    id,
                    value: ValueSnapshots { original, current },
                    operations,
                },
            ));
        }
    }

    records
}
