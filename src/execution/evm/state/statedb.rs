//! REVM database interface for the EVM state cache.

use std::convert::Infallible;

use alloy_evm::revm::{
    Database,
    state::{AccountInfo, Bytecode},
};
use alloy_primitives::{Address, B256, U256};

use super::EvmStateCache;

impl Database for EvmStateCache {
    type Error = Infallible;

    /// Return account information unless the account is absent or deleted.
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.inner.inner.get(&address).and_then(|account| {
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
            .inner
            .inner
            .values()
            .filter(|account| account.info.code_hash == code_hash)
            .find_map(|account| account.info.code.clone())
            .unwrap_or_default())
    }

    /// Return a slot's present value, or zero for absent accounts and slots.
    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let Some(account) = self.inner.inner.get(&address) else {
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
