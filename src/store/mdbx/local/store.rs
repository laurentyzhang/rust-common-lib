use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use alloy_evm::revm::{
    DatabaseRef,
    primitives::KECCAK_EMPTY,
    state::{AccountInfo, Bytecode, EvmState},
};
use alloy_primitives::{Address, B256, U256};
use reth_db::{
    Database,
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    init_db,
    mdbx::DatabaseArguments,
    tables::{Bytecodes, CanonicalHeaders, PlainAccountState, PlainStorageState},
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{Account as RethAccount, Bytecode as RethBytecode, StorageEntry};

use crate::execution::CrdtSlot;
use crate::execution::evm::state::store::merge;
use crate::execution::evm::state::{StateDb, StateOverlay};
use crate::execution::workload::ID;
use crate::namespace::crdt::CrdtKeyCodec;
use crate::store::cache::{ByWeight, CacheKey, CacheVal, PersistentStore, load};
use crate::store::traits::Committer;

use crate::store::{PhysicalStore, StoreError};

/// EVM state in MDBX, in reth's tables and codec; the cache loads its misses from it.
#[derive(Debug)]
pub struct LocalMdbxStore {
    db: reth_db::DatabaseEnv,
}

impl LocalMdbxStore {
    /// Opens, or creates, the database at `path` with reth's tables.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db = init_db(path, DatabaseArguments::default())
            .map_err(|error| StoreError::Database(error.to_string()))?;
        Ok(Self { db })
    }

    /// Writes `staged` in one transaction.
    pub(super) fn write(&self, staged: &[Staged]) -> Result<(), StoreError> {
        let tx = self.db.tx_mut().map_err(database_error)?;
        {
            let mut slots = tx
                .cursor_dup_write::<PlainStorageState>()
                .map_err(database_error)?;
            for account in staged {
                let address = account.address;
                if account.wipe && slots.seek_exact(address).map_err(database_error)?.is_some() {
                    slots.delete_current_duplicates().map_err(database_error)?;
                }
                let Some(info) = account.account else {
                    tx.delete::<PlainAccountState>(address, None)
                        .map_err(database_error)?;
                    continue;
                };
                tx.put::<PlainAccountState>(address, info)
                    .map_err(database_error)?;
                if let Some((hash, code)) = &account.code {
                    tx.put::<Bytecodes>(*hash, code.clone())
                        .map_err(database_error)?;
                }
                for entry in &account.slots {
                    // A DupSort value is replaced by deleting the old entry first.
                    if slots
                        .seek_by_key_subkey(address, entry.key)
                        .map_err(database_error)?
                        .is_some_and(|stored| stored.key == entry.key)
                    {
                        slots.delete_current().map_err(database_error)?;
                    }
                    if !entry.value.is_zero() {
                        slots.upsert(address, entry).map_err(database_error)?;
                    }
                }
            }
        }
        tx.commit().map_err(database_error)
    }
}

/// One account's changes in reth's types, staged for MDBX.
#[derive(Debug)]
pub struct Staged {
    pub(super) address: Address,
    /// Created or self-destructed: its stored slots are deleted first.
    pub(super) wipe: bool,
    /// `None` removes the account.
    pub(super) account: Option<RethAccount>,
    pub(super) code: Option<(B256, RethBytecode)>,
    /// A zero value deletes the slot.
    pub(super) slots: Vec<StorageEntry>,
}

/// Encodes `state`, already in the compact format, into reth's types.
pub(super) fn encode(state: &EvmState) -> impl Iterator<Item = Staged> + '_ {
    state.iter().map(|(&address, account)| {
        let info = &account.info;
        let has_code = info.code_hash != KECCAK_EMPTY;
        Staged {
            address,
            wipe: account.is_created() || account.is_selfdestructed(),
            account: (!account.is_selfdestructed()).then(|| RethAccount {
                nonce: info.nonce,
                balance: info.balance,
                bytecode_hash: has_code.then_some(info.code_hash),
            }),
            code: info
                .code
                .as_ref()
                .filter(|_| has_code)
                .map(|code| (info.code_hash, RethBytecode::new_raw(code.original_bytes()))),
            slots: account
                .storage
                .iter()
                .map(|(key, slot)| StorageEntry {
                    key: B256::from(key.to_be_bytes()),
                    value: slot.present_value,
                })
                .collect(),
        }
    })
}

/// Commits accepted batches to MDBX, in parallel with the cache's committer, with the same buffer,
/// stage and flush; only the formats differ. Rejected batches are never written. The CRDT values a
/// commit reads are prefetched as batches are buffered.
#[derive(Debug)]
pub struct LocalMdbxCommitter<'a> {
    buffered: BTreeMap<u64, EvmState>,
    staged: Vec<(u64, EvmState)>,
    mdbx: &'a LocalMdbxStore,
    /// The prefetch buffer's capacity in weight.
    capacity: u64,
    /// The block's prefetched CRDT values over MDBX; a value it lacks loads from MDBX.
    prefetched: PersistentStore<'a>,
}

impl<'a> LocalMdbxCommitter<'a> {
    /// Commits into `mdbx`, prefetching up to `capacity` in weight per block.
    pub fn new(mdbx: &'a LocalMdbxStore, capacity: u64) -> Self {
        Self {
            buffered: BTreeMap::new(),
            staged: Vec::new(),
            mdbx,
            capacity,
            prefetched: PersistentStore::new(capacity, ByWeight, Default::default(), Some(mdbx)),
        }
    }
}

impl Committer<u64, EvmState> for LocalMdbxCommitter<'_> {
    /// Holds a batch's changes until it is accepted or rejected, and prefetches from MDBX the
    /// values of every CRDT slot it changed; a failed read is left to the commit.
    fn buffer(&mut self, batch: u64, changes: EvmState) {
        for (&owner, account) in &changes {
            for &key in account.storage.keys() {
                if CrdtKeyCodec::decode(key).is_some() {
                    let _ = self.prefetched.storage_ref(owner, key);
                }
            }
        }
        self.buffered.insert(batch, changes);
    }

    /// Keeps the accepted batches, in batch order, and drops the rest.
    fn stage(&mut self, accepted: Vec<u64>) -> Result<(), StoreError> {
        let accepted = accepted.into_iter().collect::<HashSet<_>>();
        for (batch, changes) in std::mem::take(&mut self.buffered) {
            if accepted.contains(&batch) {
                self.staged.push((batch, changes));
            }
        }
        Ok(())
    }

    /// Merges the staged batches, applies their CRDT deltas to MDBX's committed values, read from
    /// the prefetched ones first, and writes the result in the compact format in one transaction.
    /// The prefetched values are then dropped for the next block.
    fn flush(&mut self) -> Result<(), StoreError> {
        let mut crdt = CrdtSlot::default();
        let mut merged = EvmState::default();
        for (batch, changes) in std::mem::take(&mut self.staged) {
            let id = ID {
                block: batch,
                ..Default::default()
            };
            merge(&mut merged, crdt.merge_delta_from(id, changes)?);
        }
        let mut db = StateDb::with_fallback(
            StateOverlay::new(merged, StateOverlay::copy_balance_nonce),
            Some(&self.prefetched),
        );
        let mut compacted = Vec::new();
        for (owner, sets, scalars) in crdt.drain() {
            compacted.push((owner, CrdtSlot::compact(&mut db, owner, sets, scalars)?));
        }
        let mut list = db.into_inner();
        for (owner, slots) in compacted {
            if let Some(account) = list.get_mut(&owner) {
                account.storage.extend(slots);
            }
        }
        self.mdbx.write(&encode(&list).collect::<Vec<_>>())?;
        self.prefetched =
            PersistentStore::new(self.capacity, ByWeight, Default::default(), Some(self.mdbx));
        Ok(())
    }
}

impl PhysicalStore<CacheKey, CacheVal> for LocalMdbxStore {
    /// Loads the key from MDBX in the execution format.
    fn get(&self, key: &CacheKey) -> Result<Option<CacheVal>, StoreError> {
        load(self, key)
    }
}

impl DatabaseRef for LocalMdbxStore {
    type Error = StoreError;

    /// Reads the account from `PlainAccountState`; its code is loaded by hash when needed.
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, StoreError> {
        let tx = self.db.tx().map_err(database_error)?;
        let account = tx
            .get::<PlainAccountState>(address)
            .map_err(database_error)?;
        Ok(account.map(|account| AccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
            account_id: None,
            code: None,
        }))
    }

    /// Reads the code from `Bytecodes`; missing code reads as empty.
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, StoreError> {
        let tx = self.db.tx().map_err(database_error)?;
        let code = tx.get::<Bytecodes>(code_hash).map_err(database_error)?;
        Ok(code.map_or_else(Bytecode::default, |code| {
            Bytecode::new_raw(code.0.original_bytes())
        }))
    }

    /// Reads the slot from `PlainStorageState`; a missing slot reads as zero.
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, StoreError> {
        let key = B256::from(index.to_be_bytes());
        let tx = self.db.tx().map_err(database_error)?;
        let entry = tx
            .cursor_dup_read::<PlainStorageState>()
            .map_err(database_error)?
            .seek_by_key_subkey(address, key)
            .map_err(database_error)?;
        Ok(entry
            .filter(|entry| entry.key == key)
            .map_or(U256::ZERO, |entry| entry.value))
    }

    /// Reads the block hash from `CanonicalHeaders`; a missing block reads as zero.
    fn block_hash_ref(&self, number: u64) -> Result<B256, StoreError> {
        let tx = self.db.tx().map_err(database_error)?;
        let hash = tx.get::<CanonicalHeaders>(number).map_err(database_error)?;
        Ok(hash.unwrap_or_default())
    }
}

fn database_error(error: reth_db::DatabaseError) -> StoreError {
    StoreError::Database(error.to_string())
}

#[cfg(test)]
#[path = "tests/local.rs"]
mod tests;
