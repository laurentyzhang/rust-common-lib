use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header, length_of_length};

use crate::{collections::delta_set::DeltaSet, crdt::u64_set::U64Set};

impl Encodable for U64Set {
    fn encode(&self, out: &mut dyn BufMut) {
        let payload_length: usize = self.entries.iter_live().map(Encodable::length).sum();

        Header {
            list: true,
            payload_length,
        }
        .encode(out);
        for entry in self.entries.iter_live() {
            entry.encode(out);
        }
    }

    fn length(&self) -> usize {
        let payload_length: usize = self.entries.iter_live().map(Encodable::length).sum();
        payload_length + length_of_length(payload_length)
    }
}

impl Decodable for U64Set {
    fn decode(input: &mut &[u8]) -> Result<Self, Error> {
        let mut payload = Header::decode_bytes(input, true)?;
        let mut entries = Vec::new();
        while !payload.is_empty() {
            entries.push(u64::decode(&mut payload)?);
        }

        Ok(Self {
            entries: DeltaSet::try_from_elements(entries)
                .ok_or(Error::Custom("duplicate U64Set entry"))?,
            delta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy_rlp::{decode_exact, encode};

    use crate::crdt::state::DeltaOp;

    use super::*;

    #[test]
    fn path_meta_storage_round_trip_ignores_delta() {
        let dirty = U64Set {
            entries: DeltaSet::try_from_elements(vec![10, 20, 30]).unwrap(),
            delta: Some(vec![DeltaOp::Add(40), DeltaOp::Sub(10)]),
        };

        let clean = U64Set {
            entries: dirty.entries.clone(),
            delta: None,
        };

        assert_eq!(encode(&dirty), encode(&clean));
        let encoded = encode(&dirty);
        let decoded = decode_exact::<U64Set>(&encoded).unwrap();

        assert!(decoded == clean);
        assert_eq!(dirty.length(), encoded.len());
    }
}
