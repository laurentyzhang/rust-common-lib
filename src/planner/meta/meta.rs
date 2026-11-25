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
    /// Stored conflict peers in storage order; new peers are appended after them.
    pub conflict_list: Vec<u64>,
    pub sequential_only: bool,
    pub deferred_peers: BTreeSet<u64>,
    pub deferred_gas_limit: u64,
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

    /// Loads the sequential-only flag and the stored conflict peers for one contract function.
    fn load_conflicts<DB: Database>(
        &self,
        db: &mut DB,
    ) -> Result<(bool, Vec<u64>), BlockExecutionError> {
        let flag = db
            .storage(
                self.id.account,
                MetaFieldId::ConflictCount.slot(self.id.account, self.id.selector, 0),
            )
            .map_err(BlockExecutionError::other)?;
        if flag == U256::MAX {
            return Ok((true, Vec::new()));
        }
        let peers = Self::load_peers(
            db,
            self.id.account,
            self.id.selector,
            MetaFieldId::ConflictIndex,
        )?;
        Ok((false, peers))
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
        let deferred_peers =
            Self::load_peers(db, contract, selector, MetaFieldId::DeferredIndex)?;
        let deferred_gas_limit = db
            .storage(
                contract,
                MetaFieldId::DeferredGasLimit.slot(contract, selector, 0),
            )
            .map_err(BlockExecutionError::other)?;

        let (sequential_only, conflict_list) = meta.load_conflicts(db)?;
        Ok(Self {
            conflict_peers: conflict_list.iter().copied().collect(),
            conflict_list,
            sequential_only,
            deferred_peers: deferred_peers.into_iter().collect(),
            deferred_gas_limit: Self::decode_u64(deferred_gas_limit, "deferred gas limit")?,
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

            let codec = MetaFieldId::ConflictIndex.codec(contract, selector);
            let length_slot = codec.key(0)?;
            // Start fresh when building a new update from the same loaded meta_lookup.
            if !updates
                .get(&contract)
                .is_some_and(|account| account.storage.contains_key(&length_slot))
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
            let peers = meta
                .conflict_list
                .iter()
                .chain(&meta.pending_peers)
                .copied()
                .collect::<Vec<_>>();

            // Rewrite the length and the chunks from the new peer onward; earlier chunks are unchanged.
            codec.encode_at::<_, _, u64, BlockExecutionError>(
                &peers,
                meta.conflict_list.len() + offset,
                |slot, value| {
                    let original = match account.storage.get(&slot) {
                        Some(slot) => slot.original_value(),
                        None => db.storage(contract, slot).map_err(BlockExecutionError::other)?,
                    };
                    account.storage.insert(
                        slot,
                        EvmStorageSlot::new_changed(original, value, Default::default()),
                    );
                    Ok(())
                },
            )?;
        }
        Ok(())
    }

    fn decode_u64(value: U256, field: &str) -> Result<u64, BlockExecutionError> {
        MetaCodec::decode_u64(value)
            .ok_or_else(|| BlockExecutionError::msg(format!("{field} exceeds u64")))
    }

    /// Loads a peer list stored through `field`'s slot codec, in storage order.
    fn load_peers<DB: Database>(
        db: &mut DB,
        contract: Address,
        selector: [u8; 4],
        field: MetaFieldId,
    ) -> Result<Vec<u64>, BlockExecutionError> {
        field
            .codec(contract, selector)
            .decode(|slot| db.storage(contract, slot).map_err(BlockExecutionError::other))
    }
}
