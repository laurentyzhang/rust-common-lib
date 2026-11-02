mod codec;
pub mod field;

pub use codec::MetaKeyCodec;

use std::collections::{BTreeMap, BTreeSet};

use alloy_evm::{
    block::BlockExecutionError,
    revm::{
        Database,
        state::{Account, EvmState, EvmStorageSlot},
    },
};
use alloy_primitives::{Address, U256};

use super::{Planner, resolver::ConflictResult};
use field::Property;

impl<DB: Database, K> Planner<DB, K> {
    /// Loads one function's compact conflict peers or its sequential-only marker.
    pub(super) fn load_conflicts(
        &mut self,
        host: Address,
        callee: u64,
    ) -> Result<(bool, BTreeSet<u64>), BlockExecutionError> {
        let count = self
            .db
            .storage(host, Property::ConflictCount.slot(host, callee, 0))
            .map_err(BlockExecutionError::other)?;
        if count == U256::MAX {
            return Ok((true, BTreeSet::new()));
        }
        let count: u64 = count
            .try_into()
            .map_err(|_| BlockExecutionError::msg("conflict count exceeds u64"))?;
        if count.div_ceil(4) > u64::from(u32::MAX) + 1 {
            return Err(BlockExecutionError::msg("conflict slot index exceeds u32"));
        }
        let mut peers = BTreeSet::new();
        for index in 0..count.div_ceil(4) {
            let value = self
                .db
                .storage(
                    host,
                    Property::ConflictIndex.slot(host, callee, index as u32),
                )
                .map_err(BlockExecutionError::other)?;
            for peer in value
                .to_be_bytes::<32>()
                .chunks_exact(8)
                .take((count - index * 4).min(4) as usize)
            {
                peers.insert(u64::from_le_bytes(peer.try_into().unwrap()));
            }
        }
        Ok((false, peers))
    }

    /// Builds contract metadata updates from the detected function pairs.
    pub fn resolve(
        &mut self,
        mut result: ConflictResult<K>,
    ) -> Result<ConflictResult<K>, BlockExecutionError> {
        let mut peers = BTreeMap::<(Address, u64), BTreeSet<u64>>::new();
        for &(left, right) in &result.involved_pairs {
            let left = self.callees.get(&left.global).ok_or_else(|| {
                BlockExecutionError::msg("conflicting transaction was not scheduled")
            })?;
            let right = self.callees.get(&right.global).ok_or_else(|| {
                BlockExecutionError::msg("conflicting transaction was not scheduled")
            })?;
            if let (Some(left), Some(right)) = (left, right) {
                peers.entry(*left).or_default().insert(right.1);
                peers.entry(*right).or_default().insert(left.1);
            }
        }

        let mut updates = EvmState::default();
        for ((host, callee), detected) in peers {
            let (sequential_only, existing) = self.load_conflicts(host, callee)?;
            if sequential_only {
                continue;
            }
            let mut new_peers = detected.difference(&existing).copied().peekable();
            if new_peers.peek().is_none() {
                continue;
            }
            if !updates.contains_key(&host) {
                let info = self
                    .db
                    .basic(host)
                    .map_err(BlockExecutionError::other)?
                    .unwrap_or_default();
                updates.insert(host, Account::from(info).with_touched_mark());
            }
            let account = updates.get_mut(&host).unwrap();
            let mut position = existing.len() as u64;

            // Append into the final partial slot before allocating another packed slot.
            while new_peers.peek().is_some() {
                let index = (position / 4)
                    .try_into()
                    .map_err(|_| BlockExecutionError::msg("conflict slot index exceeds u32"))?;
                let slot = Property::ConflictIndex.slot(host, callee, index);
                let original = self
                    .db
                    .storage(host, slot)
                    .map_err(BlockExecutionError::other)?;
                let mut bytes = original.to_be_bytes::<32>();
                for entry in bytes.chunks_exact_mut(8).skip((position % 4) as usize) {
                    let Some(peer) = new_peers.next() else {
                        break;
                    };
                    entry.copy_from_slice(&peer.to_le_bytes());
                    position += 1;
                }
                account.storage.insert(
                    slot,
                    EvmStorageSlot::new_changed(
                        original,
                        U256::from_be_bytes(bytes),
                        Default::default(),
                    ),
                );
            }
            account.storage.insert(
                Property::ConflictCount.slot(host, callee, 0),
                EvmStorageSlot::new_changed(
                    U256::from(existing.len()),
                    U256::from(position),
                    Default::default(),
                ),
            );
        }
        result.contract_meta = updates;
        Ok(result)
    }
}
