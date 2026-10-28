use alloy_primitives::{I256, U256};
use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header, length_of_length};

use crate::crdt::Scalar;

impl Encodable for Scalar<I256> {
    fn encode(&self, out: &mut dyn BufMut) {
        let has_value = 1_u8;
        let value = self.value.into_raw();
        let has_limits = 1_u8;
        let lower = self.min.into_raw();
        let upper = self.max.into_raw();
        let payload_length = has_value.length()
            + value.length()
            + has_limits.length()
            + lower.length()
            + upper.length();

        Header { list: true, payload_length }.encode(out);
        has_value.encode(out);
        value.encode(out);
        has_limits.encode(out);
        lower.encode(out);
        upper.encode(out);
    }

    fn length(&self) -> usize {
        let payload_length = 1_u8.length()
            + self.value.into_raw().length()
            + 1_u8.length()
            + self.min.into_raw().length()
            + self.max.into_raw().length();
        payload_length + length_of_length(payload_length)
    }
}

impl Decodable for Scalar<I256> {
    fn decode(input: &mut &[u8]) -> Result<Self, Error> {
        let mut payload = Header::decode_bytes(input, true)?;
        let has_value = u8::decode(&mut payload)?;
        let value = I256::from_raw(U256::decode(&mut payload)?);
        let has_limits = u8::decode(&mut payload)?;
        let lower = I256::from_raw(U256::decode(&mut payload)?);
        let upper = I256::from_raw(U256::decode(&mut payload)?);

        if has_value > 1 || has_limits > 1 || !payload.is_empty() {
            return Err(Error::Custom("invalid I256 encoding"));
        }

        let (min, max) = if has_limits == 1 {
            (lower, upper)
        } else {
            (I256::MIN, I256::MAX)
        };

        Ok(Self {
            value: if has_value == 1 { value } else { I256::ZERO },
            delta: None,
            min,
            max,
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy_rlp::{decode_exact, encode};

    use super::*;
    use crate::crdt::state::DeltaOp;

    #[test]
    fn i256_storage_round_trip_ignores_delta() {
        let dirty = Scalar {
            value: I256::unchecked_from(-100),
            delta: Some(DeltaOp::Add(U256::from(25))),
            min: I256::unchecked_from(-1_000),
            max: I256::unchecked_from(1_000),
        };
        let clean = Scalar { delta: None, ..dirty.clone() };

        assert_eq!(encode(&dirty), encode(&clean));
        let encoded = encode(&dirty);
        assert_eq!(decode_exact::<Scalar<I256>>(&encoded).unwrap(), clean);
        assert_eq!(dirty.length(), encoded.len());
    }
}
