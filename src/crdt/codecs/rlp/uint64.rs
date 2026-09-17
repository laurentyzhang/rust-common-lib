use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header, length_of_length};

use crate::crdt::uint64::U64;

impl Encodable for U64 {
    fn encode(&self, out: &mut dyn BufMut) {
        let has_value = 1_u8;
        let value = self.value;
        let has_limits = 1_u8;
        let (lower, upper) = self.limits;
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
        let has_value = 1_u8;
        let value = self.value;
        let has_limits = 1_u8;
        let (lower, upper) = self.limits;
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
            value: if has_value == 1 { value } else { 0 },
            delta: 0,
            delta_subtract: false,
            limits: if has_limits == 1 {
                (lower, upper)
            } else {
                (u64::MIN, u64::MAX)
            },
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
            value: 100,
            delta: 25,
            delta_subtract: false,
            limits: (0, 1_000),
        };
        let clean = U64 {
            value: dirty.value,
            delta: 0,
            delta_subtract: false,
            limits: dirty.limits,
        };

        assert_eq!(encode(&dirty), encode(&clean));
        let encoded = encode(&dirty);
        let decoded = decode_exact::<U64>(&encoded).unwrap();

        assert!(decoded == clean);
        assert_eq!(dirty.length(), encoded.len());
    }
}
