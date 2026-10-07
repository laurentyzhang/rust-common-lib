//! A revm `Database` over a `VmCache`, which falls back to its `FallbackStore` on a miss.

use std::collections::HashMap;
use std::convert::Infallible;

use alloy_evm::revm::Database;
use alloy_evm::revm::bytecode::Bytecode;
use alloy_evm::revm::state::AccountInfo;
use alloy_primitives::{Address, B256, KECCAK256_EMPTY, U256, keccak256};

use crate::execution::VmCache;

/// Cache keys for the parts of an account.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum StateKey {
    Balance(Address),
    Nonce(Address),
    Code(Address),
    Storage(Address, U256),
}

/// Reads accounts and storage through a `VmCache`. Writes are not supported yet.
pub struct StateDB<'a> {
    execution_cache: VmCache<'a, StateKey>,
    /// Code seen by `basic`, so `code_by_hash` can answer later.
    codes: HashMap<B256, Bytecode>,
}

impl<'a> StateDB<'a> {
    pub fn new(execution_cache: VmCache<'a, StateKey>) -> Self {
        Self {
            execution_cache,
            codes: HashMap::new(),
        }
    }
}

impl Database for StateDB<'_> {
    type Error = Infallible;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let balance = self
            .execution_cache
            .get(&StateKey::Balance(address))
            .and_then(|v| v.as_u256());
        let nonce = self
            .execution_cache
            .get(&StateKey::Nonce(address))
            .and_then(|v| v.as_u64());
        let code = self
            .execution_cache
            .get(&StateKey::Code(address))
            .and_then(|v| v.as_bytes().map(|b| b.to_vec()));

        if balance.is_none() && nonce.is_none() && code.is_none() {
            return Ok(None);
        }

        let (code_hash, code) = match code {
            Some(bytes) if !bytes.is_empty() => {
                let hash = keccak256(&bytes);
                let code = Bytecode::new_raw(bytes.into());
                self.codes.insert(hash, code.clone());
                (hash, Some(code))
            }
            _ => (KECCAK256_EMPTY, None),
        };

        let mut info = AccountInfo::default();
        info.balance = balance.unwrap_or_default();
        info.nonce = nonce.unwrap_or_default();
        info.code_hash = code_hash;
        info.code = code;
        Ok(Some(info))
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(self.codes.get(&code_hash).cloned().unwrap_or_default())
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        Ok(self
            .execution_cache
            .get(&StateKey::Storage(address, index))
            .and_then(|v| v.as_u256())
            .unwrap_or_default())
    }

    /// Block hashes are not kept in the cache.
    fn block_hash(&mut self, _number: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}
