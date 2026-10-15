//! A revm `Database` over a `VmStateCache`, which falls back to its `FallbackStore` on a miss.

use std::collections::HashMap;

use alloy_evm::revm::bytecode::Bytecode;
use alloy_evm::revm::state::{AccountInfo, EvmState};
use alloy_evm::revm::{Database, DatabaseCommit};
use alloy_primitives::{Address, B256, U256};

use super::account_info::{AccountDecodeError, AccountInfoCodec};
use super::key::Key;
use crate::committer::traits::TransitionWriter;
use crate::crdt::state::{DeltaOp, value::CrdtValue};
use crate::crdt::{NonComm as CrdtNonComm, Scalar};
use crate::execution::{VmStateCache, VmStateCacheWriter, cache::Entry};
// use crate::store::StoreError;

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

    #[cfg(test)]
    pub(super) fn execution_store(&mut self) -> &mut VmStateCache<'a, Key> {
        &mut self.execution_store
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

impl DatabaseCommit for StateDB<'_> {
    fn commit(&mut self, changes: EvmState) {
        let mut updates = Vec::new();

        for (address, account) in changes {
            if !account.is_touched() {
                continue;
            }

            if account.is_selfdestructed() {
                updates.push((Key::Account(address), Entry::Crdt(CrdtValue::None)));
                updates.extend(
                    account
                        .storage
                        .into_keys()
                        .map(|slot| (Key::Storage(address, slot), Entry::Crdt(CrdtValue::None))),
                );
                continue;
            }

            let encoded = account.info.encode();
            let value = CrdtNonComm::<[u8]>::new(encoded)
                .expect("encoded EVM account must be a valid byte value")
                .into();
            updates.push((Key::Account(address), Entry::Crdt(value)));

            updates.extend(account.storage.into_iter().filter_map(|(slot, value)| {
                if !value.is_changed() {
                    return None;
                }

                let present = value.present_value();
                if present.is_zero() {
                    return Some((Key::Storage(address, slot), Entry::Crdt(CrdtValue::None)));
                }

                let original = value.original_value();
                let delta = if present > original {
                    DeltaOp::Add(present - original)
                } else {
                    DeltaOp::Sub(original - present)
                };
                Some((
                    Key::Storage(address, slot),
                    Entry::Crdt(Scalar::<U256>::from_delta(delta).into()),
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
