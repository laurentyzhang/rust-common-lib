use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header, length_of_length};

use crate::crdt::uint64::U64;

impl Encodable for U64 {
    fn encode(&self, out: &mut dyn BufMut) {
        let has_value = u8::from(self.value.is_some());
        let value = self.value.unwrap_or(0);
        let has_limits = u8::from(self.limits.is_some());
        let (lower, upper) = self.limits.unwrap_or((0, 0));
        let payload_length = has_value.length()
            + value.length()
            + has_limits.length()
            + lower.length()
            + upper.length();

        Header {
            list: true,
            payload_length,
        }
        .encode(out);
        has_value.encode(out);
        value.encode(out);
        has_limits.encode(out);
        lower.encode(out);
        upper.encode(out);
    }

    fn length(&self) -> usize {
        let has_value = u8::from(self.value.is_some());
        let value = self.value.unwrap_or(0);
        let has_limits = u8::from(self.limits.is_some());
        let (lower, upper) = self.limits.unwrap_or((0, 0));
        let payload_length = has_value.length()
            + value.length()
            + has_limits.length()
            + lower.length()
            + upper.length();
        payload_length + length_of_length(payload_length)
    }
}

impl Decodable for U64 {
    fn decode(input: &mut &[u8]) -> Result<Self, Error> {
        let mut payload = Header::decode_bytes(input, true)?;
        let has_value = u8::decode(&mut payload)?;
        let value = u64::decode(&mut payload)?;
        let has_limits = u8::decode(&mut payload)?;
        let lower = u64::decode(&mut payload)?;
        let upper = u64::decode(&mut payload)?;

        if has_value > 1 || has_limits > 1 || !payload.is_empty() {
            return Err(Error::Custom("invalid U64 encoding"));
        }

        Ok(Self {
            value: (has_value == 1).then_some(value),
            delta: None,
            limits: (has_limits == 1).then_some((lower, upper)),
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy_rlp::{decode_exact, encode};

    use super::*;

    #[test]
    fn u64_storage_round_trip_ignores_delta() {
        let dirty = U64 {
            value: Some(100),
            delta: Some(25),
            limits: Some((0, 1_000)),
        };
        let clean = U64 {
            value: dirty.value,
            delta: None,
            limits: dirty.limits,
        };

        assert_eq!(encode(&dirty), encode(&clean));
        let encoded = encode(&dirty);
        let decoded = decode_exact::<U64>(&encoded).unwrap();

        assert!(decoded == clean);
        assert_eq!(dirty.length(), encoded.len());
    }
}
