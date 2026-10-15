use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header, length_of_length};

use crate::crdt::Scalar;

impl Encodable for Scalar<i64> {
    fn encode(&self, out: &mut dyn BufMut) {
        let has_value = 1_u8;
        let value = self.value;
        let value = ((value as u64) << 1) ^ ((value >> 63) as u64);
        let has_limits = 1_u8;
        let (lower, upper) = self.limits;
        let lower = ((lower as u64) << 1) ^ ((lower >> 63) as u64);
        let upper = ((upper as u64) << 1) ^ ((upper >> 63) as u64);
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
        let value = ((value as u64) << 1) ^ ((value >> 63) as u64);
        let has_limits = 1_u8;
        let (lower, upper) = self.limits;
        let lower = ((lower as u64) << 1) ^ ((lower >> 63) as u64);
        let upper = ((upper as u64) << 1) ^ ((upper >> 63) as u64);
        let payload_length = has_value.length()
            + value.length()
            + has_limits.length()
            + lower.length()
            + upper.length();
        payload_length + length_of_length(payload_length)
    }
}

impl Decodable for Scalar<i64> {
    fn decode(input: &mut &[u8]) -> Result<Self, Error> {
        let mut payload = Header::decode_bytes(input, true)?;
        let has_value = u8::decode(&mut payload)?;
        let value = u64::decode(&mut payload)?;
        let value = ((value >> 1) as i64) ^ -((value & 1) as i64);
        let has_limits = u8::decode(&mut payload)?;
        let lower = u64::decode(&mut payload)?;
        let lower = ((lower >> 1) as i64) ^ -((lower & 1) as i64);
        let upper = u64::decode(&mut payload)?;
        let upper = ((upper >> 1) as i64) ^ -((upper & 1) as i64);

        if has_value > 1 || has_limits > 1 || !payload.is_empty() {
            return Err(Error::Custom("invalid I64 encoding"));
        }

        Ok(Self {
            value: if has_value == 1 { value } else { 0 },
            delta: None,
            limits: if has_limits == 1 {
                (lower, upper)
            } else {
                (i64::MIN, i64::MAX)
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy_rlp::{decode_exact, encode};

    use super::*;

    #[test]
    fn i64_storage_round_trip_ignores_delta() {
        let dirty = Scalar {
            value: -100,
            delta: Some(crate::crdt::state::DeltaOp::Add(25)),
            limits: (-1_000, 1_000),
        };

        let clean = Scalar {
            value: dirty.value,
            delta: None,
            limits: dirty.limits,
        };

        assert_eq!(encode(&dirty), encode(&clean));
        let encoded = encode(&dirty);
        let decoded = decode_exact::<Scalar<i64>>(&encoded).unwrap();

        assert!(decoded == clean);
        assert_eq!(dirty.length(), encoded.len());
    }
}
