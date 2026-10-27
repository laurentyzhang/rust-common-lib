use alloy_evm::revm::state::{EvmState, EvmStorage, EvmStorageSlot};
use alloy_primitives::{U256, map::hash_map::Entry};
use rayon::prelude::*;

use crate::execution::evm::{ApiAddress, Field, SlotCodec};

/// An owned EVM state with ordered parallel merging.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeableEvmState {
    pub inner: EvmState,
}

impl MergeableEvmState {
    /// Consume an existing EVM state without copying its accounts or storage.
    pub fn new(state: EvmState) -> Self {
        Self { inner: state }
    }

    /// Insert new or replacement accounts, then merge overlapping accounts in parallel.
    /// New slots insert directly. Existing scalar deltas accumulate changes;
    /// other existing API slots retain their values.
    /// Incoming values win for ordinary slots and account deletion records.
    pub fn merge(&mut self, other: Self) {
        // An empty destination can take the entire incoming map without copying it.
        if self.inner.is_empty() {
            *self = other;
            return;
        }

        // Insert new accounts directly and send overlapping accounts to parallel workers.
        let mut accts_to_merge = Vec::with_capacity(self.inner.len().min(other.inner.len()));
        for (address, incoming_account) in other.inner {
            match self.inner.entry(address) {
                Entry::Occupied(entry)
                    if !incoming_account.is_created()
                        && !incoming_account.is_selfdestructed()
                        && !entry.get().is_selfdestructed() =>
                {
                    // Add the both existing and incoming accounts to the list.
                    accts_to_merge.push((address, entry.remove(), incoming_account));
                }
                entry => {
                    // Add new accounts directly to the map.
                    entry.insert_entry(incoming_account);
                }
            }
        }

        let merged_accts: Vec<_> = accts_to_merge
            .into_par_iter()
            .map(|(owner_addr, mut old_acct, mut new_acct)| {
                // Preserve creation status when later updates replace account metadata.
                if old_acct.is_created() {
                    new_acct.mark_created();
                }

                for (key, new_value) in new_acct.storage {
                    if !SlotCodec::is_api_key(key) || !old_acct.storage.contains_key(&key) {
                        old_acct.storage.insert(key, new_value);
                    } else {
                        Self::handle_api_slot(&mut old_acct.storage, key, new_value);
                    }
                }

                // Keep the combined storage alongside the incoming account metadata.
                new_acct.storage = old_acct.storage;
                (owner_addr, new_acct)
            })
            .collect();
        // Update the destination map sequentially after the parallel merges finish.
        self.inner.extend(merged_accts);
    }

    /// Dispatches CRDT delta merging by API type.
    fn handle_api_slot(storage: &mut EvmStorage, key: U256, new_value: EvmStorageSlot) {
        match SlotCodec::decode(key) {
            Some((ApiAddress::Scalar, field, _)) if field == Field::Delta as u8 => {
                // Apply only this state's change to the encoded delta.
                let change = new_value
                    .present_value()
                    .wrapping_sub(new_value.original_value());
                let slot = storage
                    .get_mut(&key)
                    .expect("API slot exists before handling");
                slot.present_value = slot.present_value().wrapping_add(change);
            }
            _ => {}
        }
    }
}

impl From<MergeableEvmState> for EvmState {
    /// Recover the owned state without copying it.
    fn from(state: MergeableEvmState) -> Self {
        state.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_evm::revm::state::{Account, EvmStorageSlot};
    use alloy_primitives::{Address, U256};

    fn state(slots: &[(U256, U256, U256)]) -> MergeableEvmState {
        let mut account = Account::default();
        for &(key, original, present) in slots {
            account.storage.insert(
                key,
                EvmStorageSlot::new_changed(original, present, Default::default()),
            );
        }
        MergeableEvmState::new(EvmState::from_iter([(Address::ZERO, account)]))
    }

    #[test]
    fn scalar_merge_accumulates_changes_and_ignores_other_scalar_fields() {
        let keys = [Field::Min, Field::Max, Field::Value, Field::Delta]
            .map(|field| SlotCodec::encode(ApiAddress::Scalar, field as u8, Address::ZERO));
        let zero = U256::ZERO;
        for (original, existing, incoming, expected) in [
            (
                U256::from(10),
                U256::from(15),
                U256::from(13),
                U256::from(18),
            ),
            (
                U256::from(10),
                U256::from(15),
                U256::from(7),
                U256::from(12),
            ),
            (
                U256::from(10),
                U256::from(15),
                U256::from(10),
                U256::from(15),
            ),
            (zero, U256::from(5), U256::MAX, U256::from(4)),
        ] {
            let mut merged = state(&[
                (keys[0], zero, zero),
                (keys[1], zero, U256::from(100)),
                (keys[2], zero, U256::from(50)),
                (keys[3], original, existing),
            ]);
            let original_slots = merged.inner[&Address::ZERO].storage.clone();
            merged.merge(state(&[
                (keys[0], zero, U256::from(20)),
                (keys[1], zero, U256::from(200)),
                (keys[2], zero, U256::from(70)),
                (keys[3], original, incoming),
            ]));

            let slots = &merged.inner[&Address::ZERO].storage;
            for key in &keys[..3] {
                assert_eq!(slots[key], original_slots[key]);
            }
            assert_eq!(slots[&keys[3]].present_value(), expected);
            assert_eq!(slots[&keys[3]].original_value(), original);
        }
    }

    #[test]
    fn scalar_delta_merge_handles_parallel_groups_and_sequential_snapshots() {
        let key = SlotCodec::encode(ApiAddress::Scalar, Field::Delta as u8, Address::ZERO);
        let baseline = U256::from(10);
        let mut left = state(&[(key, baseline, U256::from(12))]);
        left.merge(state(&[(key, baseline, U256::from(13))]));
        let mut right = state(&[(key, baseline, U256::from(9))]);
        right.merge(state(&[(key, baseline, U256::from(14))]));
        left.merge(right);
        assert_eq!(
            left.inner[&Address::ZERO].storage[&key].present_value(),
            U256::from(18)
        );

        let mut sequential = state(&[(key, baseline, U256::from(12))]);
        sequential.merge(state(&[(key, U256::from(12), U256::from(15))]));
        assert_eq!(
            sequential.inner[&Address::ZERO].storage[&key].present_value(),
            U256::from(15)
        );
    }

    #[test]
    fn merge_inserts_new_deltas_and_preserves_ordinary_slot_overwrites() {
        let delta = SlotCodec::encode(ApiAddress::Scalar, Field::Delta as u8, Address::ZERO);
        let foreign = SlotCodec::encode(ApiAddress::Container, Field::Delta as u8, Address::ZERO);
        let unknown = SlotCodec::encode(ApiAddress::Scalar, 4, Address::ZERO);
        let mut malformed = delta.to_be_bytes::<32>();
        malformed[0] ^= 0xff;
        let malformed = U256::from_be_bytes(malformed);
        let keys = [U256::from(7), foreign, unknown, malformed];
        let mut merged = state(&keys.map(|key| (key, U256::ZERO, U256::from(99))));
        let mut incoming = state(&keys.map(|key| (key, U256::from(99), U256::ZERO)));
        let delta_slot =
            EvmStorageSlot::new_changed(U256::from(5), U256::from(8), Default::default());
        incoming
            .inner
            .get_mut(&Address::ZERO)
            .unwrap()
            .storage
            .insert(delta, delta_slot.clone());
        merged.merge(incoming);

        let slots = &merged.inner[&Address::ZERO].storage;
        assert_eq!(slots[&delta], delta_slot);
        for key in [U256::from(7), malformed] {
            assert_eq!(slots[&key].present_value(), U256::ZERO);
        }
        for key in [foreign, unknown] {
            assert_eq!(slots[&key].present_value(), U256::from(99));
        }
    }
}
