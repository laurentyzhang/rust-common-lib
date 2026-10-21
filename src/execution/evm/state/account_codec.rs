use std::fmt;

use alloy_evm::revm::{bytecode::Bytecode, database_interface::DBErrorMarker, state::AccountInfo};
use alloy_primitives::{B256, U256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountDecodeError;

impl fmt::Display for AccountDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid encoded AccountInfo")
    }
}

impl std::error::Error for AccountDecodeError {}
impl DBErrorMarker for AccountDecodeError {}

const ACCOUNT_HEADER_LEN: usize = 32 + 8 + 32 + 1;

pub trait AccountInfoCodec: Sized {
    fn encode(&self) -> Vec<u8>;

    fn decode(encoded: &[u8]) -> Result<Self, AccountDecodeError>;
}

impl AccountInfoCodec for AccountInfo {
    /// Encodes an account as balance, nonce, code hash, code-presence, and bytecode.
    fn encode(&self) -> Vec<u8> {
        let code = self.code.as_ref().map(Bytecode::original_bytes);
        let code_len = code.as_ref().map_or(0, |bytes| bytes.len());
        let mut encoded = Vec::with_capacity(ACCOUNT_HEADER_LEN + code_len);

        encoded.extend_from_slice(&self.balance.to_be_bytes::<32>());
        encoded.extend_from_slice(&self.nonce.to_be_bytes());
        encoded.extend_from_slice(self.code_hash.as_slice());
        encoded.push(u8::from(code.is_some()));
        if let Some(code) = code {
            encoded.extend_from_slice(&code);
        }
        encoded
    }

    fn decode(encoded: &[u8]) -> Result<Self, AccountDecodeError> {
        if encoded.len() < ACCOUNT_HEADER_LEN {
            return Err(AccountDecodeError);
        }

        let code = match encoded[72] {
            0 if encoded.len() == ACCOUNT_HEADER_LEN => None,
            1 => Some(Bytecode::new_raw(
                encoded[ACCOUNT_HEADER_LEN..].to_vec().into(),
            )),
            _ => return Err(AccountDecodeError),
        };

        Ok(Self {
            balance: U256::from_be_slice(&encoded[..32]),
            nonce: u64::from_be_bytes(encoded[32..40].try_into().expect("fixed nonce length")),
            code_hash: B256::from_slice(&encoded[40..72]),
            account_id: None,
            code,
        })
    }
}
