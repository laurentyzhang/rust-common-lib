use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header, length_of_length};

use crate::{collections::delta_set::DeltaSet, crdt::path_meta::PathMeta};

impl Encodable for PathMeta {
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

impl Decodable for PathMeta {
    fn decode(input: &mut &[u8]) -> Result<Self, Error> {
        let mut payload = Header::decode_bytes(input, true)?;
        let mut entries = Vec::new();
        while !payload.is_empty() {
            entries.push(u64::decode(&mut payload)?);
        }

        Ok(Self {
            entries: DeltaSet::try_from_elements(entries)
                .ok_or(Error::Custom("duplicate PathMeta entry"))?,
            delta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy_rlp::{decode_exact, encode};

    use crate::crdt::path_meta::PathDelta;

    use super::*;

    #[test]
    fn path_meta_storage_round_trip_ignores_delta() {
        let dirty = PathMeta {
            entries: DeltaSet::try_from_elements(vec![10, 20, 30]).unwrap(),
            delta: Some(PathDelta {
                added: vec![40],
                removed: vec![10],
            }),
        };
        let clean = PathMeta {
            entries: dirty.entries.clone(),
            delta: None,
        };

        assert_eq!(encode(&dirty), encode(&clean));
        let encoded = encode(&dirty);
        let decoded = decode_exact::<PathMeta>(&encoded).unwrap();

        assert!(decoded == clean);
        assert_eq!(dirty.length(), encoded.len());
    }
}
