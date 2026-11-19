use std::collections::{BTreeMap, BTreeSet};

use alloy_evm::{
    block::BlockExecutionError,
    revm::{
        Database,
        state::{Account, EvmState, EvmStorageSlot},
    },
};
use alloy_primitives::{Address, U256};

use crate::planner::LoadedMetadata;

use super::{CalleeId, MetaCodec, MetaFieldId};

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Meta {
    pub id: CalleeId,
    pub fully_parallelizable: bool,
    pub is_deferred: bool,
    pub conflict_peers: BTreeSet<u64>,
    /// Original peer count, used as the next append position and the old count in the update.
    pub conflict_count: u64,
    /// Original value of the last packed slot, reused when appending to a partial slot.
    pub last_conflict_slot: U256,
    pub sequential_only: bool,
    pub deferred_peers: BTreeSet<u64>,
    pub deferred_gas: u64,
}

impl Meta {
    /// Looks up a callee by ID, which sorts before the other metadata fields.
    pub(in crate::planner) fn get_by_id(metadata: &LoadedMetadata, id: CalleeId) -> Option<&Self> {
        let lower_bound = Self {
            id,
            ..Self::default()
        };
        metadata
            .range(lower_bound..)
            .next()
            .filter(|meta| meta.id == id)
    }

    /// Loads conflict policy and peers for one contract function.
    fn load_conflicts<DB: Database>(
        &self,
        db: &mut DB,
    ) -> Result<(bool, BTreeSet<u64>, u64, U256), BlockExecutionError> {
        let count = db
            .storage(
                self.id.account,
                MetaFieldId::ConflictCount.slot(self.id.account, self.id.selector, 0),
            )
            .map_err(BlockExecutionError::other)?;
        if count == U256::MAX {
            return Ok((true, BTreeSet::new(), 0, U256::ZERO));
        }
        let count = Self::decode_u64(count, "conflict count")?;
        let (peers, last_slot) = Self::load_peers(
            db,
            self.id.account,
            self.id.selector,
            count,
            MetaFieldId::ConflictIndex,
        )?;
        Ok((false, peers, count, last_slot))
    }

    /// Loads all metadata for one contract function.
    pub fn load<DB: Database>(
        db: &mut DB,
        contract: Address,
        selector: [u8; 4],
    ) -> Result<Self, BlockExecutionError> {
        // create an instance and populate its conflict and deferred metadata from storage.
        let meta = Self {
            id: CalleeId::new(contract, selector),
            ..Self::default()
        };
        let deferred_count = db
            .storage(
                contract,
                MetaFieldId::DeferredCount.slot(contract, selector, 0),
            )
            .map_err(BlockExecutionError::other)?;
        let (deferred_peers, _) = Self::load_peers(
            db,
            contract,
            selector,
            Self::decode_u64(deferred_count, "deferred count")?,
            MetaFieldId::DeferredIndex,
        )?;
        let deferred_gas = db
            .storage(
                contract,
                MetaFieldId::DeferredGas.slot(contract, selector, 0),
            )
            .map_err(BlockExecutionError::other)?;

        let (sequential_only, conflict_peers, conflict_count, last_conflict_slot) =
            meta.load_conflicts(db)?;
        Ok(Self {
            conflict_peers,
            conflict_count,
            last_conflict_slot,
            sequential_only,
            deferred_peers,
            deferred_gas: Self::decode_u64(deferred_gas, "deferred gas")?,
            ..meta
        })
    }

    /// Builds storage updates that append new conflict peers to each function's metadata.
    pub(in crate::planner) fn build_conflict_peer_storage_updates<DB: Database>(
        db: &mut DB,
        peers: BTreeMap<Meta, BTreeSet<u64>>,
        current: &EvmState,
    ) -> Result<EvmState, BlockExecutionError> {
        let mut updates = EvmState::default();
        for (meta, detected) in peers {
            let contract = meta.id.account;
            let selector = meta.id.selector;
            // Do not write metadata for a contract deleted by the current execution.
            if current
                .get(&contract)
                .is_some_and(|account| account.is_selfdestructed())
            {
                continue;
            }
            // The sequential-only marker already supersedes individual peer entries.
            if meta.sequential_only {
                continue;
            }
            let mut new_peers = detected
                .difference(&meta.conflict_peers)
                .copied()
                .peekable();
            if new_peers.peek().is_none() {
                continue;
            }
            // Reuse current account information when available; storage remains
            // a set of changes rather than a copy of the account's existing slots.
            if !updates.contains_key(&contract) {
                let info = match current.get(&contract) {
                    Some(account) => account.info.clone(),
                    None => db
                        .basic(contract)
                        .map_err(BlockExecutionError::other)?
                        .unwrap_or_default(),
                };
                updates.insert(contract, Account::from(info).with_touched_mark());
            }
            let account = updates.get_mut(&contract).unwrap();
            let mut position = meta.conflict_count;

            // Each index slot holds four 8-byte peers. Append to a partial slot
            // without disturbing its existing entries, then continue in new slots.
            while new_peers.peek().is_some() {
                let index = (position / 4)
                    .try_into()
                    .map_err(|_| BlockExecutionError::msg("conflict slot index exceeds u32"))?;
                let slot = MetaFieldId::ConflictIndex.slot(contract, selector, index);
                let original = if position % 4 != 0 {
                    meta.last_conflict_slot
                } else {
                    db.storage(contract, slot)
                        .map_err(BlockExecutionError::other)?
                };
                let mut slot_peers = MetaCodec::decode_peer_slot(original);
                for entry in slot_peers.iter_mut().skip((position % 4) as usize) {
                    let Some(peer) = new_peers.next() else {
                        break;
                    };
                    *entry = peer;
                    position += 1;
                }
                account.storage.insert(
                    slot,
                    EvmStorageSlot::new_changed(
                        original,
                        MetaCodec::encode_peer_slot(slot_peers),
                        Default::default(),
                    ),
                );
            }
            // Record the old and new counts so the storage change has the correct base.
            account.storage.insert(
                MetaFieldId::ConflictCount.slot(contract, selector, 0),
                EvmStorageSlot::new_changed(
                    MetaCodec::encode_u64(meta.conflict_count),
                    MetaCodec::encode_u64(position),
                    Default::default(),
                ),
            );
        }
        Ok(updates)
    }

    fn decode_u64(value: U256, field: &str) -> Result<u64, BlockExecutionError> {
        MetaCodec::decode_u64(value)
            .ok_or_else(|| BlockExecutionError::msg(format!("{field} exceeds u64")))
    }

    fn load_peers<DB: Database>(
        db: &mut DB,
        contract: Address,
        selector: [u8; 4],
        count: u64,
        field: MetaFieldId,
    ) -> Result<(BTreeSet<u64>, U256), BlockExecutionError> {
        let slot_count = count.div_ceil(4);
        if slot_count > u64::from(u32::MAX) + 1 {
            return Err(BlockExecutionError::msg("metadata slot index exceeds u32"));
        }
        let mut peers = BTreeSet::new();
        let mut last_slot = U256::ZERO;
        for index in 0..slot_count {
            last_slot = db
                .storage(contract, field.slot(contract, selector, index as u32))
                .map_err(BlockExecutionError::other)?;
            for peer in MetaCodec::decode_peer_slot(last_slot)
                .into_iter()
                .take((count - index * 4).min(4) as usize)
            {
                peers.insert(peer);
            }
        }
        Ok((peers, last_slot))
    }
}
