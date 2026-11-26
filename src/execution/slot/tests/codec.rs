use std::collections::HashMap;

use alloy_evm::revm::state::{EvmState, EvmStorageSlot};
use alloy_primitives::{Address, U256};

use super::Codec;
use crate::common::codec::{CodecError, Decodable, Encodable};
use crate::common::width::Width;

/// Variable-length test value.
#[derive(Debug, PartialEq)]
struct Blob(Vec<u8>);

impl Encodable for Blob {
    /// Returns the raw bytes.
    fn encode(&self) -> Vec<u8> {
        self.0.clone()
    }
}

impl Decodable for Blob {
    /// Accepts any bytes.
    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        Ok(Self(bytes.to_vec()))
    }
}

/// Fixed 4-byte test value; other lengths are invalid.
#[derive(Debug, PartialEq)]
struct Word4([u8; 4]);

impl Decodable for Word4 {
    /// Decodes exactly 4 bytes.
    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        Ok(Self(bytes.try_into().map_err(|_| CodecError::Invalid)?))
    }
}

/// Storage failure distinct from codec failures, to check pass-through.
#[derive(Debug, PartialEq)]
enum StoreError {
    Unavailable,
    Codec(CodecError),
}

impl From<CodecError> for StoreError {
    /// Wraps a codec failure.
    fn from(error: CodecError) -> Self {
        Self::Codec(error)
    }
}

/// Encodes `value` into a map-backed store.
fn store(codec: &Codec, value: &Blob) -> Result<HashMap<U256, U256>, CodecError> {
    let mut slots = HashMap::new();
    codec.encode(value, |key, word| {
        slots.insert(key, word);
        Ok::<_, CodecError>(())
    })?;
    Ok(slots)
}

/// Decodes a `T` from a map-backed store; missing slots read as zero.
fn load<T: Decodable>(codec: &Codec, slots: &HashMap<U256, U256>) -> Result<T, CodecError> {
    codec.decode(|key| Ok(slots.get(&key).copied().unwrap_or_default()))
}

#[test]
fn round_trips_across_chunk_boundaries() {
    let codec = Codec::new(vec![0xAA; 8], vec![]);
    for len in [0usize, 1, 31, 32, 33, 100] {
        // Trailing zeros must survive, so the stored length is what bounds the data.
        let value = Blob((0..len).map(|i| if i + 1 == len { 0 } else { i as u8 + 1 }).collect());
        let slots = store(&codec, &value).unwrap();

        assert_eq!(slots.len(), 1 + len.div_ceil(32), "len {len}");
        assert_eq!(load::<Blob>(&codec, &slots).unwrap(), value, "len {len}");
    }
}

#[test]
fn keys_carry_prefix_and_index_with_length_first() {
    let prefix = vec![0xAA; 8];
    let codec = Codec::new(prefix.clone(), vec![]);
    let slots = store(&codec, &Blob(vec![7; 33])).unwrap();

    let key = |index: u8| {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&prefix);
        bytes[31] = index;
        U256::from_be_bytes(bytes)
    };
    assert_eq!(slots[&key(0)], U256::from(33));
    assert_eq!(slots[&key(1)], U256::from_be_bytes([7; 32]));

    // The last chunk is zero-padded on the right.
    let mut last = [0; 32];
    last[0] = 7;
    assert_eq!(slots[&key(2)], U256::from_be_bytes(last));
}

#[test]
fn rejects_keys_that_overflow() {
    assert_eq!(
        store(&Codec::new(vec![0; 32], vec![]), &Blob(vec![])),
        Err(CodecError::KeyOverflow)
    );

    // A 1-byte index holds indexes 0..=255: length plus 255 chunks.
    let codec = Codec::new(vec![0; 31], vec![]);
    assert!(store(&codec, &Blob(vec![1; 255 * 32])).is_ok());
    assert_eq!(
        store(&codec, &Blob(vec![1; 256 * 32])),
        Err(CodecError::KeyOverflow)
    );
}

#[test]
fn rejects_invalid_stored_data() {
    let codec = Codec::new(vec![0xAA; 8], vec![]);

    // Bytes that the target type can't decode.
    let slots = store(&codec, &Blob(vec![1; 5])).unwrap();
    assert_eq!(load::<Word4>(&codec, &slots), Err(CodecError::Invalid));
    let slots = store(&codec, &Blob(vec![1, 2, 3, 4])).unwrap();
    assert_eq!(load::<Word4>(&codec, &slots), Ok(Word4([1, 2, 3, 4])));

    // A length word too large for usize.
    let length_key = *store(&codec, &Blob(vec![])).unwrap().keys().next().unwrap();
    let slots = HashMap::from([(length_key, U256::MAX)]);
    assert_eq!(load::<Blob>(&codec, &slots), Err(CodecError::Invalid));
}

#[test]
fn storage_errors_pass_through_unchanged() {
    let codec = Codec::new(vec![0xAA; 8], vec![]);

    let written = codec.encode(&Blob(vec![1]), |_, _| Err(StoreError::Unavailable));
    assert_eq!(written, Err(StoreError::Unavailable));

    let read = codec.decode::<Blob, _>(|_| Err(StoreError::Unavailable));
    assert_eq!(read, Err(StoreError::Unavailable));

    // Codec failures still reach the caller through its own error type.
    let overflow = Codec::new(vec![0; 32], vec![]).decode::<Blob, _>(|_| Ok::<_, StoreError>(U256::ZERO));
    assert_eq!(overflow, Err(StoreError::Codec(CodecError::KeyOverflow)));
}

#[test]
fn suffix_follows_index_and_narrows_it() {
    let codec = Codec::new(vec![0xAA; 12], vec![0xBB; 16]);
    let mut expected = [0; 32];
    expected[..12].copy_from_slice(&[0xAA; 12]);
    expected[12..16].copy_from_slice(&7u32.to_be_bytes());
    expected[16..].copy_from_slice(&[0xBB; 16]);
    assert_eq!(codec.key(7), Ok(U256::from_be_bytes(expected)));

    // The index has 4 bytes, so indexes from 2^32 overflow.
    assert!(codec.key(u64::from(u32::MAX)).is_ok());
    assert_eq!(codec.key(1 << 32), Err(CodecError::KeyOverflow));
    assert_eq!(
        Codec::new(vec![0; 16], vec![0; 16]).key(0),
        Err(CodecError::KeyOverflow)
    );
}

#[test]
fn encode_at_writes_length_and_chunks_from_element_onward() {
    let codec = Codec::new(vec![0xAA; 8], vec![]);
    let peers: Vec<u64> = (1..=9).collect();
    let mut written = Vec::new();
    codec
        .encode_at::<_, _, u64, CodecError>(&peers, 5, |key, _| {
            written.push(key);
            Ok(())
        })
        .unwrap();

    // Element 5 sits in chunk 1 (elements 4..8), so chunk 0 is skipped.
    let keys = [0, 2, 3].map(|index| codec.key(index).unwrap());
    assert_eq!(written, keys);
}

#[test]
fn decode_at_reads_one_element_including_across_chunks() {
    let codec = Codec::new(vec![0xAA; 8], vec![]);
    let peers: Vec<u64> = (1..=9).collect();
    let mut slots = HashMap::new();
    codec
        .encode(&peers, |key, word| {
            slots.insert(key, word);
            Ok::<_, CodecError>(())
        })
        .unwrap();
    assert_eq!(load::<Vec<u64>>(&codec, &slots).unwrap(), peers);

    // Element 1 lives in chunk 0 only: length plus one chunk are read.
    let mut reads = Vec::new();
    let peer = codec
        .decode_at::<_, u64, CodecError>(1, |key| {
            reads.push(key);
            Ok(slots[&key])
        })
        .unwrap();
    assert_eq!(peer, 2);
    assert_eq!(reads, [0, 1].map(|index| codec.key(index).unwrap()));

    // A 20-byte element at index 1 spans bytes 20..40, so chunks 0 and 1 are read.
    let blobs = vec![Bytes20([1; 20]), Bytes20([2; 20])];
    let mut slots = HashMap::new();
    codec
        .encode(&blobs, |key, word| {
            slots.insert(key, word);
            Ok::<_, CodecError>(())
        })
        .unwrap();
    assert_eq!(
        codec.decode_at::<_, Bytes20, CodecError>(1, |key| Ok(slots[&key])),
        Ok(Bytes20([2; 20]))
    );

    // Past the stored length is invalid.
    assert_eq!(
        codec.decode_at::<_, Bytes20, CodecError>(2, |key| Ok(slots[&key])),
        Err(CodecError::Invalid)
    );
}

/// Fixed 20-byte test element.
#[derive(Debug, PartialEq)]
struct Bytes20([u8; 20]);

impl Encodable for Bytes20 {
    /// Returns the raw bytes.
    fn encode(&self) -> Vec<u8> {
        self.0.to_vec()
    }
}

impl Decodable for Bytes20 {
    /// Decodes exactly 20 bytes.
    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        Ok(Self(bytes.try_into().map_err(|_| CodecError::Invalid)?))
    }
}

impl Width<20> for Bytes20 {}

/// Writes `word` to `owner`'s slot `key` in `state` as a changed slot, keeping any earlier original value.
fn write_state(
    state: &mut EvmState,
    owner: Address,
    key: U256,
    word: U256,
) -> Result<(), CodecError> {
    let storage = &mut state.entry(owner).or_default().storage;
    let original = storage.get(&key).map_or(U256::ZERO, |slot| slot.original_value());
    storage.insert(key, EvmStorageSlot::new_changed(original, word, Default::default()));
    Ok(())
}

/// Reads `owner`'s slot `key` from `state`; missing accounts and slots read as zero.
fn read_state(state: &EvmState, owner: Address, key: U256) -> Result<U256, CodecError> {
    Ok(state
        .get(&owner)
        .and_then(|account| account.storage.get(&key))
        .map_or(U256::ZERO, |slot| slot.present_value()))
}

#[test]
fn round_trips_through_evm_state() {
    let (owner, other) = (Address::repeat_byte(1), Address::repeat_byte(2));
    let peers_codec = Codec::new(vec![0xAA; 12], vec![0xBB; 16]);
    let words_codec = Codec::new(vec![0xCC; 12], vec![0xBB; 16]);
    let peers: Vec<u64> = (1..=9).collect();
    let words: Vec<U256> = [5, 0, 7].map(U256::from).to_vec();

    let mut state = EvmState::default();
    peers_codec
        .encode(&peers, |key, word| write_state(&mut state, owner, key, word))
        .unwrap();
    words_codec
        .encode(&words, |key, word| write_state(&mut state, owner, key, word))
        .unwrap();

    // Both values live in the owner's account only, under their own keys.
    assert!(!state.contains_key(&other));
    assert_eq!(
        peers_codec.decode::<Vec<u64>, _>(|key| read_state(&state, owner, key)),
        Ok(peers)
    );
    assert_eq!(
        words_codec.decode::<Vec<U256>, _>(|key| read_state(&state, owner, key)),
        Ok(words)
    );

    // Another account sees nothing: an empty list.
    assert_eq!(
        peers_codec.decode::<Vec<u64>, _>(|key| read_state(&state, other, key)),
        Ok(vec![])
    );
}

#[test]
fn encode_at_and_decode_at_update_evm_state_in_place() {
    let owner = Address::repeat_byte(1);
    let codec = Codec::new(vec![0xAA; 12], vec![0xBB; 16]);
    let mut state = EvmState::default();
    codec
        .encode(&(1..=5).collect::<Vec<u64>>(), |key, word| {
            write_state(&mut state, owner, key, word)
        })
        .unwrap();
    let first_chunk = read_state(&state, owner, codec.key(1).unwrap()).unwrap();

    // Appending element 5 (value 6) rewrites the length and chunk 1 only; chunk 0 is untouched.
    let mut written = Vec::new();
    codec
        .encode_at::<_, _, u64, CodecError>(&(1..=6).collect::<Vec<u64>>(), 5, |key, word| {
            written.push(key);
            write_state(&mut state, owner, key, word)
        })
        .unwrap();
    assert_eq!(written, [0, 2].map(|index| codec.key(index).unwrap()));
    assert_eq!(read_state(&state, owner, codec.key(1).unwrap()), Ok(first_chunk));

    // The length slot keeps its first-write original and shows the new byte length.
    let length = &state[&owner].storage[&codec.key(0).unwrap()];
    assert_eq!(length.original_value(), U256::ZERO);
    assert_eq!(length.present_value(), U256::from(6 * 8));

    assert_eq!(
        codec.decode::<Vec<u64>, _>(|key| read_state(&state, owner, key)),
        Ok((1..=6).collect())
    );
    assert_eq!(
        codec.decode_at::<_, u64, _>(5, |key| read_state(&state, owner, key)),
        Ok(6)
    );
    assert_eq!(
        codec.decode_at::<_, u64, _>(6, |key| read_state(&state, owner, key)),
        Err(CodecError::Invalid)
    );
}
