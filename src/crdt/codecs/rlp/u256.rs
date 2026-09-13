use alloy_primitives::U256 as AlloyU256;
use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header, length_of_length};

use crate::crdt::u256::U256;

impl Encodable for U256 {
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

impl Decodable for U256 {
    fn decode(input: &mut &[u8]) -> Result<Self, Error> {
        let mut payload = Header::decode_bytes(input, true)?;
        let has_value = u8::decode(&mut payload)?;
        let value = AlloyU256::decode(&mut payload)?;
        let has_limits = u8::decode(&mut payload)?;
        let lower = AlloyU256::decode(&mut payload)?;
        let upper = AlloyU256::decode(&mut payload)?;

        if has_value > 1 || has_limits > 1 || !payload.is_empty() {
            return Err(Error::Custom("invalid U256 encoding"));
        }

        Ok(Self {
            value: if has_value == 1 {
                value
            } else {
                AlloyU256::ZERO
            },
            delta: AlloyU256::ZERO,
            limits: if has_limits == 1 {
                (lower, upper)
            } else {
                (AlloyU256::ZERO, AlloyU256::MAX)
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy_rlp::{decode_exact, encode};

    use super::*;

    #[test]
    fn u256_storage_round_trip_ignores_delta() {
        let dirty = U256 {
            value: AlloyU256::from(1_u64) << 200,
            delta: AlloyU256::from(25),
            limits: (AlloyU256::ZERO, AlloyU256::MAX),
        };
        let clean = U256 {
            value: dirty.value,
            delta: AlloyU256::ZERO,
            limits: dirty.limits,
        };

        assert_eq!(encode(&dirty), encode(&clean));
        let encoded = encode(&dirty);
        let decoded = decode_exact::<U256>(&encoded).unwrap();

        assert!(decoded == clean);
        assert_eq!(dirty.length(), encoded.len());
    }
}
