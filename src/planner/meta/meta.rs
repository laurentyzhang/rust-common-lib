use std::collections::{BTreeMap, BTreeSet};

use alloy_evm::{
    block::BlockExecutionError,
    revm::{
        Database,
        state::{Account, EvmState, EvmStorageSlot},
    },
};
use alloy_primitives::{Address, U256};

use super::{CalleeId, MetaCodec, MetaFieldId};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Meta {
    pub id: Option<CalleeId>,
    pub fully_parallelizable: bool,
    pub is_deferred: bool,
    pub conflict_peers: BTreeSet<u64>,
    pub sequential_only: bool,
    pub deferred_peers: BTreeSet<u64>,
    pub deferred_gas: u64,
}

impl Meta {
    /// Loads conflict policy and peers for one contract function.
    fn load_conflicts<DB: Database>(
        db: &mut DB,
        contract: Address,
        selector: [u8; 4],
    ) -> Result<(bool, BTreeSet<u64>), BlockExecutionError> {
        let count = db
            .storage(
                contract,
                MetaFieldId::ConflictCount.slot(contract, selector, 0),
            )
            .map_err(BlockExecutionError::other)?;
        if count == U256::MAX {
            return Ok((true, BTreeSet::new()));
        }
        Ok((
            false,
            Self::load_peers(
                db,
                contract,
                selector,
                Self::decode_u64(count, "conflict count")?,
                MetaFieldId::ConflictIndex,
            )?,
        ))
    }

    /// Loads all stored metadata for one contract function.
    pub fn load<DB: Database>(
        db: &mut DB,
        contract: Address,
        selector: [u8; 4],
    ) -> Result<Self, BlockExecutionError> {
        let deferred_count = db
            .storage(
                contract,
                MetaFieldId::DeferredCount.slot(contract, selector, 0),
            )
            .map_err(BlockExecutionError::other)?;
        let deferred_peers = Self::load_peers(
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

        let (sequential_only, conflict_peers) = Self::load_conflicts(db, contract, selector)?;
        Ok(Self {
            id: Some(CalleeId::new(contract, selector)),
            conflict_peers,
            sequential_only,
            deferred_peers,
            deferred_gas: Self::decode_u64(deferred_gas, "deferred gas")?,
            ..Self::default()
        })
    }

    /// Builds storage updates that append new conflict peers to each function's metadata.
    pub(in crate::planner) fn updates<DB: Database>(
        db: &mut DB,
        peers: BTreeMap<(Address, [u8; 4]), BTreeSet<u64>>,
        current: &EvmState,
    ) -> Result<EvmState, BlockExecutionError> {
        let mut updates = EvmState::default();
        for ((contract, selector), detected) in peers {
            // Do not write metadata for a contract deleted by the current execution.
            if current
                .get(&contract)
                .is_some_and(|account| account.is_selfdestructed())
            {
                continue;
            }
            let (sequential_only, existing) = Self::load_conflicts(db, contract, selector)?;
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
            let mut position = existing.len() as u64;

            // Each index slot holds four 8-byte peers. Append to a partial slot
            // without disturbing its existing entries, then continue in new slots.
            while new_peers.peek().is_some() {
                let index = (position / 4)
                    .try_into()
                    .map_err(|_| BlockExecutionError::msg("conflict slot index exceeds u32"))?;
                let slot = MetaFieldId::ConflictIndex.slot(contract, selector, index);
                let original = db
                    .storage(contract, slot)
                    .map_err(BlockExecutionError::other)?;
                let mut slot_peers = MetaCodec::decode_peer_slot(original);
                for entry in slot_peers.iter_mut().skip((position % 4) as usize) {
                    let Some(peer) = new_peers.next() else {
                        break;
                    };
                    *entry = peer;
                    position += 1;
                }
                // Record the old and new counts so the storage change has the correct base.
                account.storage.insert(
                    slot,
                    EvmStorageSlot::new_changed(
                        original,
                        MetaCodec::encode_peer_slot(slot_peers),
                        Default::default(),
                    ),
                );
            }
            account.storage.insert(
                MetaFieldId::ConflictCount.slot(contract, selector, 0),
                EvmStorageSlot::new_changed(
                    MetaCodec::encode_u64(existing.len() as u64),
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
    ) -> Result<BTreeSet<u64>, BlockExecutionError> {
        let slot_count = count.div_ceil(4);
        if slot_count > u64::from(u32::MAX) + 1 {
            return Err(BlockExecutionError::msg("metadata slot index exceeds u32"));
        }
        let mut peers = BTreeSet::new();
        for index in 0..slot_count {
            let value = db
                .storage(contract, field.slot(contract, selector, index as u32))
                .map_err(BlockExecutionError::other)?;
            for peer in MetaCodec::decode_peer_slot(value)
                .into_iter()
                .take((count - index * 4).min(4) as usize)
            {
                peers.insert(peer);
            }
        }
        Ok(peers)
    }
}
