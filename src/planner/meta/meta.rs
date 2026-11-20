use std::collections::BTreeSet;

use alloy_evm::{
    block::BlockExecutionError,
    revm::{
        Database,
        state::{Account, EvmState, EvmStorageSlot},
    },
};
use alloy_primitives::{Address, U256};

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
    /// Sorted peer IDs staged for the current conflict meta_lookup update.
    pub pending_peers: Vec<u64>,
}

impl Meta {
    /// Looks up a callee by ID, which sorts before the other meta_lookup fields.
    pub(in crate::planner) fn get_by_id(
        meta_lookup: &BTreeSet<Meta>,
        id: CalleeId,
    ) -> Option<&Self> {
        let lower_bound = Self {
            id,
            ..Self::default()
        };
        meta_lookup
            .range(lower_bound..)
            .next()
            .filter(|meta| meta.id == id)
    }

    /// Takes a callee out of the set, so its pending peers can be updated.
    pub(in crate::planner) fn take_by_id(
        meta_lookup: &mut BTreeSet<Meta>,
        id: CalleeId,
    ) -> Option<Self> {
        let lower_bound = Self {
            id,
            ..Self::default()
        };
        meta_lookup
            .extract_if(lower_bound.., |meta| meta.id == id)
            .next()
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

    /// Loads all meta_lookup for one contract function.
    pub fn load<DB: Database>(
        db: &mut DB,
        contract: Address,
        selector: [u8; 4],
    ) -> Result<Self, BlockExecutionError> {
        // create an instance and populate its conflict and deferred meta_lookup from storage.
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

    /// Records both directions of a conflict and stages the changed storage slots.
    pub(in crate::planner) fn add_conflict_peers<DB: Database>(
        &mut self,
        right: Option<&mut Meta>,
        db: &mut DB,
        current: &EvmState,
        updates: &mut EvmState,
    ) -> Result<(), BlockExecutionError> {
        // A missing right entry means both jobs call the same function.
        let left_peer = right
            .as_ref()
            .map_or(self.id.compact, |right| right.id.compact);
        let right_peer = self.id.compact;
        for (meta, peer) in [(Some(self), left_peer), (right, right_peer)] {
            let Some(meta) = meta else {
                continue;
            };
            let contract = meta.id.account;
            let selector = meta.id.selector;
            let current_account = current.get(&contract);
            if meta.sequential_only
                || meta.conflict_peers.contains(&peer)
                || current_account.is_some_and(|account| account.is_selfdestructed())
            {
                continue;
            }

            let count_slot = MetaFieldId::ConflictCount.slot(contract, selector, 0);
            // Start fresh when building a new update from the same loaded meta_lookup.
            if !updates
                .get(&contract)
                .is_some_and(|account| account.storage.contains_key(&count_slot))
            {
                meta.pending_peers.clear();
            }
            let offset = match meta.pending_peers.binary_search(&peer) {
                Ok(_) => continue,
                Err(offset) => offset,
            };
            if !updates.contains_key(&contract) {
                let info = match current_account {
                    Some(account) => account.info.clone(),
                    None => db
                        .basic(contract)
                        .map_err(BlockExecutionError::other)?
                        .unwrap_or_default(),
                };
                updates.insert(contract, Account::from(info).with_touched_mark());
            }
            let account = updates.get_mut(&contract).unwrap();
            meta.pending_peers.insert(offset, peer);
            let mut position = meta.conflict_count + offset as u64;
            let mut new_peers = &meta.pending_peers[offset..];

            // Repack the affected suffix in peer ID order, reusing staged slots.
            while !new_peers.is_empty() {
                let index = (position / 4)
                    .try_into()
                    .map_err(|_| BlockExecutionError::msg("conflict slot index exceeds u32"))?;
                let slot = MetaFieldId::ConflictIndex.slot(contract, selector, index);
                let (original, present) = match account.storage.get(&slot) {
                    Some(slot) => (slot.original_value(), slot.present_value()),
                    None => {
                        let original = if position % 4 != 0 {
                            meta.last_conflict_slot
                        } else {
                            db.storage(contract, slot)
                                .map_err(BlockExecutionError::other)?
                        };
                        (original, original)
                    }
                };
                let start = (position % 4) as usize;
                let (peers, remaining) = new_peers.split_at(new_peers.len().min(4 - start));
                let mut slot_peers = MetaCodec::decode_peer_slot(present);
                slot_peers[start..start + peers.len()].copy_from_slice(peers);
                position += peers.len() as u64;
                new_peers = remaining;
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
                count_slot,
                EvmStorageSlot::new_changed(
                    MetaCodec::encode_u64(meta.conflict_count),
                    MetaCodec::encode_u64(position),
                    Default::default(),
                ),
            );
        }
        Ok(())
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
            return Err(BlockExecutionError::msg(
                "meta_lookup slot index exceeds u32",
            ));
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
