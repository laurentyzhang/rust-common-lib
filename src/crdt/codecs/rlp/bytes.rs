use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header, length_of_length};

use crate::crdt::bytes::Bytes;

impl Encodable for Bytes {
    fn encode(&self, out: &mut dyn BufMut) {
        let is_some = u8::from(self.delta.is_some());
        let value = self.delta.as_deref().unwrap_or(&[]);
        let payload_length = is_some.length() + value.length();

        Header {
            list: true,
            payload_length,
        }
        .encode(out);
        is_some.encode(out);
        value.encode(out);
    }

    fn length(&self) -> usize {
        let is_some = u8::from(self.delta.is_some());
        let value = self.delta.as_deref().unwrap_or(&[]);
        let payload_length = is_some.length() + value.length();
        payload_length + length_of_length(payload_length)
    }
}

impl Decodable for Bytes {
    fn decode(input: &mut &[u8]) -> Result<Self, Error> {
        let mut payload = Header::decode_bytes(input, true)?;
        let is_some = u8::decode(&mut payload)?;
        let value = Header::decode_bytes(&mut payload, false)?.to_vec();

        if is_some > 1 || !payload.is_empty() {
            return Err(Error::Custom("invalid Bytes encoding"));
        }

        Ok(Self {
            delta: (is_some == 1).then(|| value.into_boxed_slice()),
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy_rlp::{decode_exact, encode};

    use super::*;

    #[test]
    fn bytes_storage_round_trip() {
        let value = Bytes {
            delta: Some(vec![1, 2, 3].into_boxed_slice()),
        };

        let encoded = encode(&value);
        let decoded = decode_exact::<Bytes>(&encoded).unwrap();

        assert!(decoded == value);
        assert_eq!(value.length(), encoded.len());
    }
}
