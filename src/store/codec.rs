use super::StoreError;

pub trait StoreCodec<T>: Send + Sync {
    fn encode(&self, value: &T) -> Result<Vec<u8>, StoreError>;
    fn decode(&self, encoded: &[u8]) -> Result<T, StoreError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct IdentityCodec;

impl StoreCodec<Vec<u8>> for IdentityCodec {
    fn encode(&self, value: &Vec<u8>) -> Result<Vec<u8>, StoreError> {
        Ok(value.clone())
    }

    fn decode(&self, encoded: &[u8]) -> Result<Vec<u8>, StoreError> {
        Ok(encoded.to_vec())
    }
}
