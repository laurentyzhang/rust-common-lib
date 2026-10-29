//! A revm `Database` over a `VmStateCache`, which falls back to its `FallbackStore` on a miss.

// use std::collections::HashMap;

// use alloy_evm::revm::bytecode::Bytecode;
// use alloy_evm::revm::state::{AccountInfo, EvmState};
// use alloy_evm::revm::{Database, DatabaseCommit};
use alloy_primitives::{Address, U256};

// use super::account_codec::{AccountDecodeError, AccountInfoCodec};
// use crate::crdt::state::{DeltaOp, value::CrdtValue};
// use crate::crdt::{NonComm as CrdtNonComm, Scalar};
// use crate::execution::{VmStateCache, VmStateCacheWriter};
use crate::store::{StoreCodec, StoreError};

/// Cache keys for EVM accounts and storage slots.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum Key {
    Account(Address),
    Balance(Address),
    Code(Address),
    Storage(Address, U256),
    Block(u64),
}

const ACCOUNT_KEY_TAG: u8 = 0x00;
const BALANCE_KEY_TAG: u8 = 0x01;
const CODE_KEY_TAG: u8 = 0x02;
const STORAGE_KEY_TAG: u8 = 0x03;
const BLOCK_KEY_TAG: u8 = 0x04;

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyCodec;

impl StoreCodec<Key> for KeyCodec {
    fn encode(&self, key: &Key) -> Result<Vec<u8>, StoreError> {
        match key {
            Key::Account(address) => Ok(encode_tagged_address(ACCOUNT_KEY_TAG, address)),
            Key::Balance(address) => Ok(encode_tagged_address(BALANCE_KEY_TAG, address)),
            Key::Code(address) => Ok(encode_tagged_address(CODE_KEY_TAG, address)),
            Key::Storage(address, slot) => {
                let mut encoded = Vec::with_capacity(53);
                encoded.push(STORAGE_KEY_TAG);
                encoded.extend_from_slice(address.as_slice());
                encoded.extend_from_slice(&slot.to_be_bytes::<32>());
                Ok(encoded)
            }
            Key::Block(number) => {
                let mut encoded = Vec::with_capacity(9);
                encoded.push(BLOCK_KEY_TAG);
                encoded.extend_from_slice(&number.to_be_bytes());
                Ok(encoded)
            }
        }
    }

    fn decode(&self, encoded: &[u8]) -> Result<Key, StoreError> {
        let Some((&tag, payload)) = encoded.split_first() else {
            return Err(StoreError::Codec("empty EVM key".into()));
        };
        match (tag, payload.len()) {
            (ACCOUNT_KEY_TAG, 20) => Ok(Key::Account(Address::from_slice(payload))),
            (BALANCE_KEY_TAG, 20) => Ok(Key::Balance(Address::from_slice(payload))),
            (CODE_KEY_TAG, 20) => Ok(Key::Code(Address::from_slice(payload))),
            (STORAGE_KEY_TAG, 52) => Ok(Key::Storage(
                Address::from_slice(&payload[..20]),
                U256::from_be_slice(&payload[20..]),
            )),
            (BLOCK_KEY_TAG, 8) => {
                let mut bytes = [0; 8];
                bytes.copy_from_slice(payload);
                Ok(Key::Block(u64::from_be_bytes(bytes)))
            }
            _ => Err(StoreError::Codec(format!(
                "invalid EVM key tag {tag} or length {}",
                payload.len()
            ))),
        }
    }
}

fn encode_tagged_address(tag: u8, address: &Address) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(21);
    encoded.push(tag);
    encoded.extend_from_slice(address.as_slice());
    encoded
}
