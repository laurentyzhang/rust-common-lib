//! A revm `Database` over a `VmStateCache`, which falls back to its `FallbackStore` on a miss.

use std::collections::HashMap;

use alloy_evm::revm::bytecode::Bytecode;
use alloy_evm::revm::state::{AccountInfo, EvmState};
use alloy_evm::revm::{Database, DatabaseCommit};
use alloy_primitives::{Address, B256, U256};

use super::account_info::{AccountDecodeError, AccountInfoCodec};
use crate::committer::traits::TransitionWriter;
use crate::crdt::state::{DeltaOp, value::CrdtValue};
use crate::crdt::{Bytes as CrdtBytes, U256 as CrdtU256};
use crate::execution::{VmStateCache, VmStateCacheWriter};
use crate::store::{StoreCodec, StoreError};

/// Cache keys for EVM accounts and storage slots.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum Key {
    Account(Address),
    Storage(Address, U256),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyCodec;

impl StoreCodec<Key> for KeyCodec {
    fn encode(&self, value: &Key) -> Result<Vec<u8>, StoreError> {
        match value {
            Key::Account(address) => Ok(address.as_slice().to_vec()),
            Key::Storage(address, slot) => {
                let mut encoded = Vec::with_capacity(52);
                encoded.extend_from_slice(address.as_slice());
                encoded.extend_from_slice(&slot.to_be_bytes::<32>());
                Ok(encoded)
            }
        }
    }

    fn decode(&self, encoded: &[u8]) -> Result<Key, StoreError> {
        match encoded.len() {
            20 => Ok(Key::Account(Address::from_slice(encoded))),
            52 => Ok(Key::Storage(
                Address::from_slice(&encoded[..20]),
                U256::from_be_slice(&encoded[20..]),
            )),
            length => Err(StoreError::Codec(format!(
                "invalid EVM key length {length}"
            ))),
        }
    }
}

/// Reads and writes EVM state through a `VmStateCache`.
pub struct StateDB<'a> {
    execution_store: VmStateCache<'a, Key>,
    /// Code seen by `basic`, so `code_by_hash` can answer later.
    codes: HashMap<B256, Bytecode>,
}

impl<'a> StateDB<'a> {
    pub fn new(execution_store: VmStateCache<'a, Key>) -> Self {
        Self {
            execution_store,
            codes: HashMap::new(),
        }
    }
}

impl DatabaseCommit for StateDB<'_> {
    fn commit(&mut self, changes: EvmState) {
        let mut updates = Vec::new();

        for (address, account) in changes {
            if !account.is_touched() {
                continue;
            }

            if account.is_selfdestructed() {
                updates.push((Key::Account(address), CrdtValue::None));
                updates.extend(
                    account
                        .storage
                        .into_keys()
                        .map(|slot| (Key::Storage(address, slot), CrdtValue::None)),
                );
                continue;
            }

            let encoded = account.info.encode();
            let value = CrdtBytes::new(encoded)
                .expect("encoded EVM account must be a valid byte value")
                .into();
            updates.push((Key::Account(address), value));

            updates.extend(account.storage.into_iter().filter_map(|(slot, value)| {
                if !value.is_changed() {
                    return None;
                }

                let present = value.present_value();
                if present.is_zero() {
                    return Some((Key::Storage(address, slot), CrdtValue::None));
                }

                let original = value.original_value();
                let delta = if present > original {
                    DeltaOp::Add(present - original)
                } else {
                    DeltaOp::Sub(original - present)
                };
                Some((
                    Key::Storage(address, slot),
                    CrdtU256::from_delta(delta).into(),
                ))
            }));
        }

        let id = self.execution_store.id();
        let cache = std::mem::replace(&mut self.execution_store, VmStateCache::new(id));
        let mut writer = VmStateCacheWriter::new(cache);
        writer
            .flush(updates)
            .expect("failed to commit EVM state to the VM cache");
        self.execution_store = writer.into_inner();
    }
}

impl Database for StateDB<'_> {
    type Error = AccountDecodeError;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let Some(encoded) = self
            .execution_store
            .get(&Key::Account(address))
            .and_then(|value| value.as_bytes().map(<[u8]>::to_vec))
        else {
            return Ok(None);
        };

        let info = AccountInfo::decode(&encoded)?;
        if let Some(code) = &info.code {
            self.codes.insert(info.code_hash, code.clone());
        }

        Ok(Some(info))
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(self.codes.get(&code_hash).cloned().unwrap_or_default())
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        Ok(self
            .execution_store
            .get(&Key::Storage(address, index))
            .and_then(|v| v.as_u256())
            .unwrap_or_default())
    }

    /// Block hashes are not kept in the cache.
    fn block_hash(&mut self, _number: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_evm::revm::state::{Account, EvmStorageSlot};

    #[test]
    fn commit_writes_account_and_storage_to_vm_cache() {
        let address = Address::repeat_byte(0x11);
        let slot = U256::from(7);
        let storage_value = U256::from(42);
        let info = AccountInfo {
            balance: U256::from(100),
            nonce: 3,
            ..AccountInfo::default()
        };
        let account = Account::from(info.clone())
            .with_storage(
                [(
                    slot,
                    EvmStorageSlot::new_changed(U256::ZERO, storage_value, Default::default()),
                )]
                .into_iter(),
            )
            .with_touched_mark();
        let mut changes = EvmState::default();
        changes.insert(address, account);

        let mut db = StateDB::new(VmStateCache::new(9));
        db.commit(changes);

        assert_eq!(db.execution_store.id(), 9);
        assert_eq!(db.basic(address).unwrap(), Some(info));
        assert_eq!(db.storage(address, slot).unwrap(), storage_value);
        assert_eq!(db.execution_store.drain().1.len(), 2);
    }
}
