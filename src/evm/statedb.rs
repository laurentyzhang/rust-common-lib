//! A revm `Database` over a `VmStateCache`, which falls back to its `FallbackStore` on a miss.

use std::collections::HashMap;

use alloy_evm::revm::Database;
use alloy_evm::revm::bytecode::Bytecode;
use alloy_evm::revm::state::AccountInfo;
use alloy_primitives::{Address, B256, U256};

use super::account_info::{AccountDecodeError, AccountInfoCodec};
use crate::execution::VmStateCache;
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

/// Reads accounts and storage through a `VmStateCache`. Writes are not supported yet.
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
