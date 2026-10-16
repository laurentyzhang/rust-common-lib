//! A revm `Database` over a `VmStateCache`, which falls back to its `FallbackStore` on a miss.

// use std::collections::HashMap;

// use alloy_evm::revm::bytecode::Bytecode;
// use alloy_evm::revm::state::{AccountInfo, EvmState};
// use alloy_evm::revm::{Database, DatabaseCommit};
use alloy_primitives::{Address, U256};

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
    Balance(Address),
    Code(Address),
    Storage(Address, U256),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyCodec;

const BALANCE_KEY_TAG: u8 = 0x01;
const CODE_KEY_TAG: u8 = 0x02;

impl StoreCodec<Key> for KeyCodec {
    fn encode(&self, key: &Key) -> Result<Vec<u8>, StoreError> {
        match key {
            Key::Account(address) => Ok(address.as_slice().to_vec()),
            Key::Balance(address) => Ok(encode_tagged_address(BALANCE_KEY_TAG, address)),
            Key::Code(address) => Ok(encode_tagged_address(CODE_KEY_TAG, address)),
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
            21 => {
                let address = Address::from_slice(&encoded[1..]);
                match encoded[0] {
                    BALANCE_KEY_TAG => Ok(Key::Balance(address)),
                    CODE_KEY_TAG => Ok(Key::Code(address)),
                    tag => Err(StoreError::Codec(format!(
                        "invalid EVM address key tag {tag}"
                    ))),
                }
            }
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

fn encode_tagged_address(tag: u8, address: &Address) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(21);
    encoded.push(tag);
    encoded.extend_from_slice(address.as_slice());
    encoded
}
