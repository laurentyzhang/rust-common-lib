use std::{marker::PhantomData, path::Path};

use alloy_evm::revm::{
    DatabaseRef,
    database_interface::DBErrorMarker,
    primitives::KECCAK_EMPTY,
    state::{AccountInfo, Bytecode, EvmState},
};
use alloy_primitives::{Address, B256, U256};
use reth_db::{
    Database,
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    init_db,
    mdbx::{DatabaseArguments, init_db_for},
    tables::{Bytecodes, CanonicalHeaders, PlainAccountState, PlainStorageState},
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{Account as RethAccount, Bytecode as RethBytecode, StorageEntry};
use std::collections::{BTreeMap, HashSet};

use super::cache::{CacheDb, to_storage};
use super::traits::Committer;
use crate::execution::evm::state::sharded::{Sharded, update};

use super::{FallbackStore, StoreCodec, StoreError};

mod tables {
    use reth_db::{TableSet, TableType, TableViewer, table::TableInfo, tables};
    use std::fmt;

    tables! {
        table CachedState {
            type Key = Vec<u8>;
            type Value = Vec<u8>;
        }
    }
}

pub struct MdbxStore<K, V, KC, VC> {
    db: reth_db::DatabaseEnv,
    key_codec: KC,
    value_codec: VC,
    marker: PhantomData<fn(K, V)>,
}

impl<K, V, KC, VC> MdbxStore<K, V, KC, VC> {
    pub fn open(
        path: impl AsRef<Path>,
        key_codec: KC,
        value_codec: VC,
    ) -> Result<Self, StoreError> {
        let db = init_db_for::<_, tables::Tables>(path, DatabaseArguments::default())
            .map_err(|error| StoreError::Database(error.to_string()))?;
        Ok(Self {
            db,
            key_codec,
            value_codec,
            marker: PhantomData,
        })
    }
}

impl<K, V, KC, VC> FallbackStore<K, V> for MdbxStore<K, V, KC, VC>
where
    KC: StoreCodec<K>,
    VC: StoreCodec<V>,
{
    fn get(&self, key: &K) -> Result<Option<V>, StoreError> {
        let key = self.key_codec.encode(key)?;
        let tx = self.db.tx().map_err(database_error)?;
        let encoded = tx.get::<tables::CachedState>(key).map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        encoded
            .as_deref()
            .map(|value| self.value_codec.decode(value))
            .transpose()
    }
}

impl<K, V, KC, VC> MdbxStore<K, V, KC, VC>
where
    K: std::hash::Hash + Eq,
    KC: StoreCodec<K>,
    VC: StoreCodec<V>,
{
    pub fn flush(&mut self, updates: Vec<(K, V)>) -> Result<(), StoreError> {
        let encoded = updates
            .iter()
            .map(|(key, value)| Ok((self.key_codec.encode(key)?, self.value_codec.encode(value)?)))
            .collect::<Result<Vec<_>, StoreError>>()?;

        let tx = self.db.tx_mut().map_err(database_error)?;
        for (key, value) in encoded {
            tx.put::<tables::CachedState>(key, value)
                .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }
}

/// EVM state in MDBX, in reth's tables and codec; the cache loads its misses from it.
#[derive(Debug)]
pub struct MdbxState {
    db: reth_db::DatabaseEnv,
}

impl MdbxState {
    /// Opens, or creates, the database at `path` with reth's tables.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db = init_db(path, DatabaseArguments::default())
            .map_err(|error| StoreError::Database(error.to_string()))?;
        Ok(Self { db })
    }

    /// Writes `staged` in one transaction.
    fn write(&self, staged: &[Staged]) -> Result<(), StoreError> {
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
struct Staged {
    address: Address,
    /// Created or self-destructed: its stored slots are deleted first.
    wipe: bool,
    /// `None` removes the account.
    account: Option<RethAccount>,
    code: Option<(B256, RethBytecode)>,
    /// A zero value deletes the slot.
    slots: Vec<StorageEntry>,
}

/// Encodes `state`, already in the compact format, into reth's types.
fn encode(state: &EvmState) -> impl Iterator<Item = Staged> + '_ {
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

/// Commits accepted blocks to MDBX: compacts their execution-format changes, encodes them into
/// reth's types, and writes them on flush. It keeps its own execution-format mirror of the
/// Sets, so compaction never reads the execution side's cache.
#[derive(Debug)]
pub struct MdbxCommitter<'a> {
    mdbx: &'a MdbxState,
    /// The Sets in the execution format, with the list MDBX holds; loaded from MDBX on a miss.
    mirror: Sharded,
    buffered: BTreeMap<u64, EvmState>,
    staged: Vec<Staged>,
}

impl<'a> MdbxCommitter<'a> {
    /// Commits into `mdbx`, starting with an empty mirror.
    pub fn new(mdbx: &'a MdbxState) -> Self {
        Self {
            mdbx,
            mirror: Sharded::new(),
            buffered: BTreeMap::new(),
            staged: Vec::new(),
        }
    }

    /// Compacts one block's changes against the mirror, stages them encoded, and updates the
    /// mirror.
    fn compact(&mut self, changes: EvmState) -> Result<(), StoreError> {
        let compacted = {
            let mirror = CacheDb::new(&self.mirror, self.mdbx);
            // A Set is expanded only once its account is loaded.
            for &owner in changes.keys() {
                mirror.basic_ref(owner)?;
            }
            to_storage(changes.clone(), &mirror)?.into_inner()
        };
        self.staged.extend(encode(&compacted));
        update(&mut self.mirror, changes, compacted);
        Ok(())
    }
}

impl Committer<u64, EvmState> for MdbxCommitter<'_> {
    /// Holds a block's execution-format changes until it is accepted.
    fn buffer(&mut self, block: u64, state: EvmState) {
        self.buffered.insert(block, state);
    }

    /// Compacts and stages the accepted blocks in block order; drops the rest.
    fn stage(&mut self, accepted: Vec<u64>) -> Result<(), StoreError> {
        let accepted = accepted.into_iter().collect::<HashSet<_>>();
        for (block, changes) in std::mem::take(&mut self.buffered) {
            if accepted.contains(&block) {
                self.compact(changes)?;
            }
        }
        Ok(())
    }

    /// Writes the staged changes in one transaction, then clears them.
    fn flush(&mut self) -> Result<(), StoreError> {
        self.mdbx.write(&self.staged)?;
        self.staged.clear();
        Ok(())
    }
}

impl DatabaseRef for MdbxState {
    type Error = StoreError;

    /// Reads the account from `PlainAccountState`; its code is loaded by hash when needed.
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, StoreError> {
        let tx = self.db.tx().map_err(database_error)?;
        let account = tx.get::<PlainAccountState>(address).map_err(database_error)?;
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

impl DBErrorMarker for StoreError {}

fn database_error(error: reth_db::DatabaseError) -> StoreError {
    StoreError::Database(error.to_string())
}

#[cfg(test)]
#[path = "tests/mdbx.rs"]
mod tests;
