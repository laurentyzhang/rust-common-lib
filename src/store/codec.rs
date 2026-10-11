use super::StoreError;
use crate::crdt::{
    codecs::internal,
    state::{Numeric, value::CrdtValue},
};

pub trait StoreCodec<T>: Send + Sync {
    fn encode(&self, value: &T) -> Result<Vec<u8>, StoreError>;
    fn decode(&self, encoded: &[u8]) -> Result<T, StoreError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct IdentityCodec;

#[derive(Clone, Copy, Debug, Default)]
pub struct CrdtValueCodec;

impl StoreCodec<Vec<u8>> for IdentityCodec {
    fn encode(&self, value: &Vec<u8>) -> Result<Vec<u8>, StoreError> {
        Ok(value.clone())
    }

    fn decode(&self, encoded: &[u8]) -> Result<Vec<u8>, StoreError> {
        Ok(encoded.to_vec())
    }
}

impl<'a> StoreCodec<CrdtValue<'a>> for CrdtValueCodec {
    fn encode(&self, value: &CrdtValue<'a>) -> Result<Vec<u8>, StoreError> {
        let (tag, payload) = match value {
            CrdtValue::None => (0, Ok(Vec::new())),
            CrdtValue::Bytes(value) => (1, internal::bytes::encode(value.as_ref())),
            CrdtValue::Numeric(Numeric::I64(value)) => {
                (2, internal::int64::encode(value.as_ref()))
            }
            CrdtValue::Numeric(Numeric::U64(value)) => {
                (3, internal::uint64::encode(value.as_ref()))
            }
            CrdtValue::Numeric(Numeric::U256(value)) => {
                (4, internal::u256::encode(value.as_ref()))
            }
            CrdtValue::U64Set(value) => (5, internal::u64_set::encode(value.as_ref())),
        };
        let mut encoded = Vec::with_capacity(payload.as_ref().map_or(1, |bytes| bytes.len() + 1));
        encoded.push(tag);
        encoded.extend(payload.map_err(codec_error)?);
        Ok(encoded)
    }

    fn decode(&self, encoded: &[u8]) -> Result<CrdtValue<'a>, StoreError> {
        let (&tag, payload) = encoded
            .split_first()
            .ok_or_else(|| StoreError::Codec("empty CRDT value".to_owned()))?;
        match tag {
            0 if payload.is_empty() => Ok(CrdtValue::None),
            0 => Err(StoreError::Codec("None value has a payload".to_owned())),
            1 => internal::bytes::decode(payload)
                .map(CrdtValue::from)
                .map_err(codec_error),
            2 => internal::int64::decode(payload)
                .map(CrdtValue::from)
                .map_err(codec_error),
            3 => internal::uint64::decode(payload)
                .map(CrdtValue::from)
                .map_err(codec_error),
            4 => internal::u256::decode(payload)
                .map(CrdtValue::from)
                .map_err(codec_error),
            5 => internal::u64_set::decode(payload)
                .map(CrdtValue::from)
                .map_err(codec_error),
            _ => Err(StoreError::Codec(format!("unknown CRDT value tag {tag}"))),
        }
    }
}

fn codec_error(error: &'static str) -> StoreError {
    StoreError::Codec(error.to_owned())
}
