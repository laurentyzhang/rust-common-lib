//! REVM database adapter for an owned EVM state.

use alloy_evm::revm::{
    Database, DatabaseRef,
    state::{AccountInfo, Bytecode},
};
use alloy_primitives::{Address, B256, U256};

use super::StateDb;

impl<DB: DatabaseRef> Database for StateDb<DB> {
    type Error = DB::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.basic_ref(address)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.code_by_hash_ref(code_hash)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.storage_ref(address, index)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.block_hash_ref(number)
    }
}

impl<DB: DatabaseRef> DatabaseRef for StateDb<DB> {
    type Error = DB::Error;

    /// Return account information unless the account is absent or deleted.
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        if let Some(account) = self.inner().get(&address) {
            if account.is_selfdestructed()
                || (account.is_loaded_as_not_existing() && !account.is_created())
            {
                return Ok(None);
            }
            return Ok(Some(account.info.clone()));
        }
        match &self.fallback {
            Some(db) => db.basic_ref(address),
            None => Ok(None),
        }
    }

    /// Find embedded bytecode before consulting the fallback.
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        if let Some(code) = self
            .inner()
            .values()
            .filter(|account| account.info.code_hash == code_hash)
            .find_map(|account| account.info.code.clone())
        {
            return Ok(code);
        }
        match &self.fallback {
            Some(db) => db.code_by_hash_ref(code_hash),
            None => Ok(Bytecode::default()),
        }
    }

    /// Local values and account resets take precedence over fallback storage.
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        if let Some(account) = self.inner().get(&address) {
            if account.is_selfdestructed()
                || (account.is_loaded_as_not_existing() && !account.is_created())
            {
                return Ok(U256::ZERO);
            }
            if let Some(slot) = account.storage.get(&index) {
                return Ok(slot.present_value());
            }
            if account.is_created() {
                return Ok(U256::ZERO);
            }
        }
        match &self.fallback {
            Some(db) => db.storage_ref(address, index),
            None => Ok(U256::ZERO),
        }
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        match &self.fallback {
            Some(db) => db.block_hash_ref(number),
            None => Ok(B256::ZERO),
        }
    }
}

#[cfg(test)]
#[path = "tests/database.rs"]
mod tests;
