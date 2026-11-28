use alloy_evm::revm::state::{EvmState, EvmStorage};

use crate::execution::CrdtSlot;
use crate::execution::evm::state::store::merge;
use crate::execution::evm::state::{StateDb, StateOverlay};
use crate::namespace::crdt::CrdtKeyCodec;
use crate::store::StoreError;
use crate::store::cache::{PersistentStore, entries};

use super::store::{LocalMdbxStore, encode};

/// Writes accepted changes to MDBX, then to the cache in front of it, so the cache never holds
/// what MDBX doesn't.
#[derive(Debug)]
pub struct LocalMdbxWriter<'a> {
    pub mdbx: &'a LocalMdbxStore,
    pub cache: PersistentStore<'a>,
}

impl LocalMdbxWriter<'_> {
    /// Merges the accepted batches in order, has the CRDT slots converted to the compact format
    /// against the cache, which holds what MDBX holds, writes MDBX in one transaction, then the
    /// cache with the changes and what MDBX now holds.
    pub fn write(&mut self, staged: Vec<(u64, EvmState)>) -> Result<(), StoreError> {
        let mut merged = EvmState::default();
        for (_, changes) in staged {
            merge(&mut merged, changes);
        }
        let mut list = merged.clone();
        let mut db = StateDb::with_fallback(
            StateOverlay::new(merged, StateOverlay::copy_balance_nonce),
            Some(&self.cache),
        );
        // CRDT slots go to the CRDT router; native slots are stored as is.
        for (&owner, account) in list.iter_mut() {
            let mut crdt = EvmStorage::default();
            account.storage.retain(|&key, slot| match CrdtKeyCodec::decode(key) {
                Some(_) => {
                    crdt.insert(key, std::mem::take(slot));
                    false
                }
                None => true,
            });
            if !crdt.is_empty() {
                account
                    .storage
                    .extend(CrdtSlot::compact(&mut db, owner, &crdt)?);
            }
        }
        let encoded = encode(&list).collect::<Vec<_>>();
        let mut cached = entries(db.into_inner());
        cached.extend(entries(list));
        self.mdbx.write(&encoded)?;
        self.cache.write(cached)
    }
}
