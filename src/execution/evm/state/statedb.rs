//! Cached EVM state and the REVM database interface.

use std::convert::Infallible;

use alloy_evm::revm::{
    Database,
    state::{Account, AccountInfo, Bytecode, EvmState},
};
use alloy_primitives::{Address, B256, U256};

use super::MergeableEvmState;

/// A cache for EVM state that supports parallel merging and provides a `Database` interface.
#[derive(Debug, Default)]
pub struct EvmStateCache {
    pub(super) inner: MergeableEvmState,
}

impl EvmStateCache {
    /// Create a cache owning the supplied state.
    pub fn new(state: EvmState) -> Self {
        Self {
            inner: MergeableEvmState::new(state),
        }
    }

    /// Return the cached account at the requested address.
    pub fn get(&self, address: &Address) -> Option<&Account> {
        self.inner.inner.get(address)
    }

    /// Iterate over the cached accounts.
    pub(crate) fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.inner.inner.values()
    }

    /// Consume the cache and return its accumulated state for persistence.
    pub fn into_evm_state(self) -> EvmState {
        self.inner.into()
    }
}

/// Implement the `Database` trait for `EvmStateCache` to provide EVM with account and
/// storage access.
impl Database for EvmStateCache {
    type Error = Infallible;

    /// Return account information unless the account is absent or deleted.
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.get(&address).and_then(|account| {
            if account.is_selfdestructed()
                || (account.is_loaded_as_not_existing() && !account.is_created())
            {
                return None;
            }
            Some(account.info.clone())
        }))
    }

    /// Find embedded bytecode by hash, returning empty code when unavailable.
    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(self
            .accounts()
            .filter(|account| account.info.code_hash == code_hash)
            .find_map(|account| account.info.code.clone())
            .unwrap_or_default())
    }

    /// Return a slot's present value, or zero for absent accounts and slots.
    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let Some(account) = self.get(&address) else {
            return Ok(U256::ZERO);
        };
        if account.is_selfdestructed()
            || (account.is_loaded_as_not_existing() && !account.is_created())
        {
            return Ok(U256::ZERO);
        }
        Ok(account
            .storage
            .get(&index)
            .map(|slot| slot.present_value())
            .unwrap_or_default())
    }

    /// EVM state contains no block history, so block hashes default to zero.
    fn block_hash(&mut self, _number: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}
