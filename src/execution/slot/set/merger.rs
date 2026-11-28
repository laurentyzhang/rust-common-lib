//! Merges Set slots: accepted states' pending changes into the shared pending changes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

use alloy_evm::revm::state::EvmStorage;
use alloy_primitives::map::{HashMap, HashSet};
use alloy_primitives::{Address, U256};

use super::{KeyCodec, Property};
use crate::common::codec::CodecError;
use crate::store::SlotStore;
use crate::execution::slot::{ApplyDelta, database};
use crate::execution::workload::ID;
use crate::store::StoreError;

/// Merges Sets by union; conflict detection leaves only independent deltas over one base.
pub struct Applier;

/// Pending Set changes of one owner, collected from accepted states as each one's own changes over
/// its base, until they are written or applied.
#[derive(Debug, Default)]
pub struct Delta {
    /// Uncancelled additions per set, by (block, generation, sequence, job), each in insertion order.
    pub(super) additions: HashMap<[u8; 6], BTreeMap<(u64, u64, u64, u64), Vec<U256>>>,
    /// Removals of committed elements per set, each element once.
    pub(super) removals: HashMap<[u8; 6], HashSet<U256>>,
    /// The `Added` keys of the base's pending additions that were cancelled.
    cancelled: HashSet<U256>,
    /// The `Removed` keys of the base's pending removals that were taken back.
    restored: HashSet<U256>,
}

impl ApplyDelta for Applier {
    type Delta = Delta;

    /// Keeps the state's own changes over its base, read from the flags it changed: an `Added`
    /// position set is an addition and one cleared a cancelled base addition; a `Removed` flag set
    /// is a removal and one cleared a restored base removal.
    fn merge_delta_from(delta: &mut Delta, id: ID, slots: &EvmStorage) -> Result<(), CodecError> {
        // A log word before and after the state; a word the state didn't write reads as zero.
        let word = |key: U256| {
            slots.get(&key).map_or((U256::ZERO, U256::ZERO), |slot| {
                (slot.original_value, slot.present_value)
            })
        };
        // `Removed` flags are keyed by hash; the removals log words written name their elements.
        let mut named = HashMap::<U256, U256>::default();
        for (&key, slot) in slots {
            if let Some((set, Property::PendingRemovals, _)) = KeyCodec::decode(key) {
                for element in [U256::ZERO, slot.original_value, slot.present_value] {
                    named.insert(KeyCodec::element(Property::Removed, set, element), element);
                }
            }
        }
        let mut added = HashMap::<[u8; 6], BTreeMap<U256, U256>>::default();
        for (&key, slot) in slots {
            let Some((set, property, _)) = KeyCodec::decode(key) else {
                continue;
            };
            let (before, after) = (slot.original_value, slot.present_value);
            if before == after {
                continue;
            }
            match property {
                Property::Added => {
                    let log = KeyCodec::delta(Property::DeltaAdded, set);
                    if !before.is_zero() {
                        delta.cancelled.insert(key);
                    }
                    if !after.is_zero() {
                        let at = u64::try_from(after).map_err(|_| CodecError::Invalid)?;
                        added.entry(set).or_default().insert(after, word(log.key(at)?).1);
                    }
                }
                Property::Removed if after.is_zero() => {
                    delta.restored.insert(key);
                }
                Property::Removed => {
                    let element = *named.get(&key).ok_or(CodecError::Invalid)?;
                    delta.removals.entry(set).or_default().insert(element);
                }
                _ => {}
            }
        }
        for (set, added) in added {
            delta
                .additions
                .entry(set)
                .or_default()
                .insert((id.block, id.generation, id.sequence, id.job), added.into_values().collect());
        }
        Ok(())
    }

    /// Writes the delta into the pending changes, which keep growing until the persistent store
    /// applies them: the base's additions and removals stay, less the cancelled and restored ones,
    /// and the new ones join, each element once. The committed elements stay unchanged.
    fn apply<S: SlotStore>(store: &mut S, owner: Address, mut delta: Delta) -> Result<(), StoreError>
    where
        S::Error: Debug,
    {
        let mut sets: HashSet<[u8; 6]> = delta
            .additions
            .keys()
            .chain(delta.removals.keys())
            .copied()
            .collect();
        sets.extend(
            delta
                .cancelled
                .iter()
                .chain(&delta.restored)
                .filter_map(|&key| KeyCodec::decode(key))
                .map(|(set, ..)| set),
        );
        for set in sets {
            let [log, cancelled, removals] = [
                Property::DeltaAdded,
                Property::DeltaCancelled,
                Property::PendingRemovals,
            ]
            .map(|property| KeyCodec::delta(property, set));
            let base: Vec<U256> = log.decode(|key| store.read(owner, key).map_err(database))?;
            let bits: Vec<U256> = cancelled.decode(|key| store.read(owner, key).map_err(database))?;
            let base_removals: Vec<U256> =
                removals.decode(|key| store.read(owner, key).map_err(database))?;

            // The base's uncancelled additions, then the new ones in (block, generation, sequence,
            // job) order, each element once.
            let mut seen = HashSet::<U256>::default();
            let added: Vec<U256> = base
                .iter()
                .enumerate()
                .filter(|&(at, &element)| {
                    !bits.get(at / 256).is_some_and(|bits| bits.bit(at % 256))
                        && !delta
                            .cancelled
                            .contains(&KeyCodec::element(Property::Added, set, element))
                })
                .map(|(_, &element)| element)
                .chain(
                    delta
                        .additions
                        .remove(&set)
                        .into_iter()
                        .flat_map(BTreeMap::into_values)
                        .flatten(),
                )
                .filter(|&element| seen.insert(element))
                .collect();
            let new_removals = delta.removals.remove(&set).unwrap_or_default();
            let removed: Vec<U256> = base_removals
                .iter()
                .copied()
                .filter(|&element| {
                    !delta
                        .restored
                        .contains(&KeyCodec::element(Property::Removed, set, element))
                })
                .chain(new_removals.iter().copied())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();

            // Cancelled and restored flags clear; a re-added element's flag is set again below.
            for &key in delta.cancelled.iter().chain(&delta.restored) {
                if KeyCodec::decode(key).is_some_and(|(owned, ..)| owned == set) {
                    store.write(owner, key, U256::ZERO).map_err(database)?;
                }
            }
            if !bits.is_empty() {
                for word in 0..=bits.len() as u64 {
                    store
                        .write(owner, cancelled.key(word)?, U256::ZERO)
                        .map_err(database)?;
                }
            }
            // Each log is rewritten from its first changed word; words past its new end clear.
            for (log, old, new) in [(&log, &base, &added), (&removals, &base_removals, &removed)] {
                let at = old.iter().zip(new).take_while(|(old, new)| old == new).count();
                log.encode_at::<_, _, U256, _>(new, at, |key, word| {
                    store.write(owner, key, word).map_err(database)
                })?;
                for stale in new.len()..old.len() {
                    store
                        .write(owner, log.key(stale as u64 + 1)?, U256::ZERO)
                        .map_err(database)?;
                }
            }
            let at = base.iter().zip(&added).take_while(|(old, new)| old == new).count();
            for (position, &element) in added.iter().enumerate().skip(at) {
                let flag = KeyCodec::element(Property::Added, set, element);
                store
                    .write(owner, flag, U256::from(position + 1))
                    .map_err(database)?;
            }
            for &element in &new_removals {
                let flag = KeyCodec::element(Property::Removed, set, element);
                store.write(owner, flag, U256::from(1)).map_err(database)?;
            }
        }
        Ok(())
    }
}
