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
    /// Builds conflict metadata using all job diagnostics and current account information.
    pub fn resolve<'a>(
        &mut self,
        verdicts: impl IntoIterator<Item = &'a CommitVerdict<K>>,
        current: &EvmState,
    ) -> Result<EvmState, BlockExecutionError>
    where
        K: 'a,
    {
        let mut peers = BTreeMap::<(Address, u64), BTreeSet<u64>>::new();
        for verdict in verdicts {
            let left = self.callees.get(&verdict.id.global).ok_or_else(|| {
                BlockExecutionError::msg("conflicting transaction was not scheduled")
            })?;
            for right in &verdict.peers {
                let right = self.callees.get(&right.global).ok_or_else(|| {
                    BlockExecutionError::msg("conflicting transaction was not scheduled")
                })?;
                if let (Some(left), Some(right)) = (left, right) {
                    peers.entry(*left).or_default().insert(right.1);
                    peers.entry(*right).or_default().insert(left.1);
                }
            }
        }

        let mut updates = EvmState::default();
        for ((host, callee), detected) in peers {
            if current
                .get(&host)
                .is_some_and(|account| account.is_selfdestructed())
            {
                continue;
            }
            let (sequential_only, existing) = self.load_conflicts(host, callee)?;
            if sequential_only {
                continue;
            }
            let mut new_peers = detected.difference(&existing).copied().peekable();
            if new_peers.peek().is_none() {
                continue;
            }
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
        Ok(updates)
    }
}
