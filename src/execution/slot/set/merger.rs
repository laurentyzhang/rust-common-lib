//! Merges Set slots: accepted states' pending changes into the indexed format.

use std::collections::BTreeMap;
use std::fmt::Debug;

use alloy_evm::revm::state::EvmStorage;
use alloy_primitives::map::{HashMap, HashSet};
use alloy_primitives::{Address, U256};

use super::{KeyCodec, Property};
use crate::common::codec::CodecError;
use crate::execution::evm::api_store::ApiStore;
use crate::execution::slot::{ApplyDelta, database};
use crate::execution::workload::ID;
use crate::store::StoreError;

/// Merges Sets by union; conflict detection leaves only independent deltas over one base.
pub struct Applier;

/// Pending Set changes of one owner, collected from accepted states until they are applied.
#[derive(Debug, Default)]
pub struct Delta {
    /// Uncancelled additions per set, by (block, generation, sequence, job), each in insertion order.
    additions: HashMap<[u8; 6], BTreeMap<(u64, u64, u64, u64), Vec<U256>>>,
    /// Removals of indexed elements per set, each element once.
    removals: HashMap<[u8; 6], HashSet<U256>>,
}

impl ApplyDelta for Applier {
    type Delta = Delta;

    /// Keeps the state's uncancelled additions under `id` and its removals.
    fn merge_delta_from(delta: &mut Delta, id: ID, slots: &EvmStorage) -> Result<(), CodecError> {
        let get = |key: U256| {
            slots
                .get(&key)
                .map_or(U256::ZERO, |slot| slot.present_value)
        };
        let mut sets = HashSet::<[u8; 6]>::default();
        for &key in slots.keys() {
            if let Some((set, ..)) = KeyCodec::decode(key) {
                sets.insert(set);
            }
        }
        for set in sets {
            let [added, cancelled, removals] = [
                Property::DeltaAdded,
                Property::DeltaCancelled,
                Property::PendingRemovals,
            ]
            .map(|property| KeyCodec::delta(property, set));
            let added: Vec<U256> = added.decode(|key| Ok::<_, CodecError>(get(key)))?;
            let cancelled: Vec<U256> = cancelled.decode(|key| Ok::<_, CodecError>(get(key)))?;
            let removals: Vec<U256> = removals.decode(|key| Ok::<_, CodecError>(get(key)))?;

            // Cancelled additions are dropped here, so their positions no longer matter.
            let live: Vec<U256> = added
                .into_iter()
                .enumerate()
                .filter(|(at, _)| {
                    !cancelled
                        .get(at / 256)
                        .is_some_and(|bits| bits.bit(at % 256))
                })
                .map(|(_, element)| element)
                .collect();
            if !live.is_empty() {
                delta
                    .additions
                    .entry(set)
                    .or_default()
                    .insert((id.block, id.generation, id.sequence, id.job), live);
            }
            if !removals.is_empty() {
                delta.removals.entry(set).or_default().extend(removals);
            }
        }
        Ok(())
    }

    /// Appends the additions in (block, generation, sequence, job) order with each element once,
    /// and turns the removals into holes; Length keeps the live count.
    fn apply<S: ApiStore>(store: &mut S, owner: Address, mut delta: Delta) -> Result<(), StoreError>
    where
        S::Error: Debug,
    {
        let sets: HashSet<[u8; 6]> = delta
            .additions
            .keys()
            .chain(delta.removals.keys())
            .copied()
            .collect();
        for set in sets {
            let length = KeyCodec::length(set);
            let mut live = store.read(owner, length).map_err(database)?;

            // Append the additions in execution order; a later copy of an element is dropped.
            let index = KeyCodec::delta(Property::Index, set);
            let mut count =
                u64::try_from(store.read(owner, index.key(0)?).map_err(database)? / U256::from(32))
                    .map_err(|_| CodecError::Invalid)?;
            let mut seen = HashSet::<U256>::default();
            let additions = delta
                .additions
                .remove(&set)
                .into_iter()
                .flat_map(BTreeMap::into_values)
                .flatten();
            for element in additions {
                if !seen.insert(element) {
                    continue;
                }
                let position = KeyCodec::element(Property::Position, set, element);
                store
                    .write(owner, index.key(count + 1)?, element)
                    .map_err(database)?;
                store
                    .write(owner, position, U256::from(count + 1))
                    .map_err(database)?;
                count += 1;
                live += U256::from(1);
            }
            store
                .write(owner, index.key(0)?, U256::from(count * 32))
                .map_err(database)?;

            // A removal leaves a hole: zero its entry, mark it in Holes, clear its Position and Node.
            let holes = KeyCodec::delta(Property::Holes, set);
            let mut holes_len = store.read(owner, holes.key(0)?).map_err(database)?;
            for element in delta.removals.remove(&set).unwrap_or_default() {
                let position = KeyCodec::element(Property::Position, set, element);
                let at = u64::try_from(store.read(owner, position).map_err(database)?)
                    .ok()
                    .and_then(|position| position.checked_sub(1))
                    .ok_or(CodecError::Invalid)?;
                store
                    .write(owner, index.key(at + 1)?, U256::ZERO)
                    .map_err(database)?;
                let word = holes.key(at / 256 + 1)?;
                let bits = store.read(owner, word).map_err(database)?
                    | (U256::from(1) << (at % 256) as usize);
                store.write(owner, word, bits).map_err(database)?;
                holes_len = holes_len.max(U256::from((at / 256 + 1) * 32));
                store.write(owner, position, U256::ZERO).map_err(database)?;
                let node = KeyCodec::element(Property::Node, set, element);
                store.write(owner, node, U256::ZERO).map_err(database)?;
                live -= U256::from(1);
            }
            store
                .write(owner, holes.key(0)?, holes_len)
                .map_err(database)?;
            store.write(owner, length, live).map_err(database)?;
        }
        Ok(())
    }
}
