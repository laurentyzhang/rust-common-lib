//! A revm `Database` over a `VmStateCache`, which falls back to its `FallbackStore` on a miss.

// use std::collections::HashMap;

// use alloy_evm::revm::bytecode::Bytecode;
// use alloy_evm::revm::state::{AccountInfo, EvmState};
// use alloy_evm::revm::{Database, DatabaseCommit};
use alloy_primitives::{Address, B256, U256};

// use super::account_info::{AccountDecodeError, AccountInfoCodec};
// use crate::committer::traits::TransitionWriter;
// use crate::crdt::state::{DeltaOp, value::CrdtValue};
// use crate::crdt::{NonComm as CrdtNonComm, Scalar};
// use crate::execution::{VmStateCache, VmStateCacheWriter};
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
