use alloy_evm::revm::context_interface::result::ResultAndState;

use crate::crdt::state::{
    Tracked,
    op::Operations,
    status::{Status, Tag},
    value::ValueSnapshots,
};
use crate::execution::evm::key::Key;

/// Exports REVM account and storage accesses without consuming the result.
pub fn delta_records<H>(
    result_and_state: &ResultAndState<H>,
    id: u64,
) -> Vec<(Key, Tracked<Status, Status>)> {
    let mut records = account_delta_records(result_and_state, id);
    records.extend(slot_delta_records(result_and_state, id));
    records
}

/// Collects account records from the result state.
fn account_delta_records<H>(
    result_and_state: &ResultAndState<H>,
    id: u64,
) -> Vec<(Key, Tracked<Status, Status>)> {
    let mut records = Vec::with_capacity(result_and_state.state.len());

    for (address, account) in &result_and_state.state {
        let mut operations = Operations::default();

        let original = if account.is_loaded_as_not_existing() || account.is_created() {
            Tag::Missing
        } else {
            Tag::Stripped
        };

        let current = if account.is_selfdestructed() {
            operations.removes.increment();
            Tag::Deleted
        } else if account.is_changed() || account.is_created() {
            operations.deltas.increment();
            if account.is_empty() {
                Tag::Default
            } else {
                Tag::Stripped
            }
        } else {
            Tag::Missing
        };

        records.push((
            Key::Account(*address),
            Tracked {
                id,
                value: ValueSnapshots {
                    original: Status::Tag(original),
                    current: Status::Tag(current),
                },
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
) -> Vec<(Key, Tracked<Status, Status>)> {
    let mut records = Vec::new();

    for (address, account) in &result_and_state.state {
        for (slot, value) in &account.storage {
            let mut operations = Operations::default();

            let original = if value.original_value().is_zero() {
                Tag::Missing
            } else {
                Tag::Stripped
            };

            let current = if !value.is_changed() {
                Tag::Missing
            } else if value.present_value().is_zero() {
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
                    value: ValueSnapshots {
                        original: Status::Tag(original),
                        current: Status::Tag(current),
                    },
                    operations,
                },
            ));
        }
    }

    records
}
