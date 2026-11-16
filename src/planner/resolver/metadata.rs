use std::collections::{BTreeMap, BTreeSet};

use alloy_evm::{
    block::BlockExecutionError,
    revm::{
        Database,
        state::{Account, EvmState, EvmStorageSlot},
    },
};
use alloy_primitives::{Address, U256};

use super::CommitVerdict;
use crate::planner::{Planner, meta::property::Property};

impl<DB: Database, K> Planner<DB, K> {
    /// Builds storage updates for the function conflicts reported by executed jobs.
    ///
    /// Transaction IDs are mapped to their scheduled `(contract, callee)` pairs.
    /// Each pair of conflicting functions is inspector_records in both directions for future
    /// scheduling. Jobs without a callee do not contribute function metadata.
    ///
    /// Existing peers are loaded from the planner database; only new peers are
    /// appended. `current` supplies account information for hosts already changed
    /// during execution and identifies selfdestructed hosts to skip. The returned
    /// state contains metadata updates; this method does not commit them.
    ///
    /// Returns an error if a reported job was not scheduled, a database lookup
    /// fails, or the existing metadata cannot be represented.
    pub fn resolve<'a>(
        &mut self,
        verdicts: impl IntoIterator<Item = &'a CommitVerdict<K>>,
        current: &EvmState,
    ) -> Result<EvmState, BlockExecutionError>
    where
        K: 'a,
    {
        // Deduplicate peers across verdicts, including conflicts reported by both jobs.
        let mut peers = BTreeMap::<(Address, u64), BTreeSet<u64>>::new();
        for verdict in verdicts {
            let left = self.callees.get(&verdict.id.global).ok_or_else(|| {
                BlockExecutionError::msg("conflicting transaction was not scheduled")
            })?;
            for right in verdict.conflicts.iter().flat_map(|(peers, _)| peers) {
                let right = self.callees.get(&right.global).ok_or_else(|| {
                    BlockExecutionError::msg("conflicting transaction was not scheduled")
                })?;
                // A job without a contract/callee cannot supply function metadata.
                if let (Some(left), Some(right)) = (left, right) {
                    peers.entry(*left).or_default().insert(right.1);
                    peers.entry(*right).or_default().insert(left.1);
                }
            }
        }

        let mut updates = EvmState::default();
        for ((host, callee), detected) in peers {
            // Do not write metadata for a host deleted by the current execution.
            if current
                .get(&host)
                .is_some_and(|account| account.is_selfdestructed())
            {
                continue;
            }
            let (sequential_only, existing) = self.load_conflicts(host, callee)?;
            // The sequential-only marker already supersedes individual peer entries.
            if sequential_only {
                continue;
            }
            let mut new_peers = detected.difference(&existing).copied().peekable();
            if new_peers.peek().is_none() {
                continue;
            }
            // Reuse current account information when available; storage remains
            // a set of changes rather than a copy of the account's existing slots.
            if !updates.contains_key(&host) {
                let info = match current.get(&host) {
                    Some(account) => account.info.clone(),
                    None => self
                        .db
                        .basic(host)
                        .map_err(BlockExecutionError::other)?
                        .unwrap_or_default(),
                };
                updates.insert(host, Account::from(info).with_touched_mark());
            }
            let account = updates.get_mut(&host).unwrap();
            let mut position = existing.len() as u64;

            // Each index slot holds four 8-byte peers. Append to a partial slot
            // without disturbing its existing entries, then continue in new slots.
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
                // Record the old and new counts so the storage change has the correct base.
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
        Ok(updates)
    }
}
