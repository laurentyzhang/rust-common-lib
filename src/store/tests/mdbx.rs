use super::*;
use crate::store::{IdentityCodec, cache::CachedStore};

#[test]
fn mdbx_writer_persists_updates_for_cached_store_reads() {
    let directory = tempfile::tempdir().unwrap();
    let mut mdbx = MdbxStore::open(directory.path(), IdentityCodec, IdentityCodec).unwrap();
    mdbx.flush(vec![(b"key".to_vec(), b"value".to_vec())])
        .unwrap();
    let store = CachedStore::new(4, Some(&mdbx));
    assert_eq!(
        store.get(&b"key".to_vec()).unwrap(),
        Some(b"value".to_vec())
    );

    drop(store);
    drop(mdbx);

    let reopened = MdbxStore::open(directory.path(), IdentityCodec, IdentityCodec).unwrap();
    assert_eq!(
        reopened.get(&b"key".to_vec()).unwrap(),
        Some(b"value".to_vec())
    );
}

#[test]
fn consume_commit_to_mdbx_and_cache_then_rebuild() {
    use crate::store::CachedStoreWriter;

    let directory = tempfile::tempdir().unwrap();
    let mut mdbx = MdbxStore::open(directory.path(), IdentityCodec, IdentityCodec).unwrap();
    let key = b"key".to_vec();

    let store = CachedStore::new(4, Some(&mdbx));
    assert_eq!(store.get(&key).unwrap(), None);
    let mut cache_writer = CachedStoreWriter::new(store.into_cache());

    let updates = vec![(key.clone(), b"value".to_vec())];
    mdbx.flush(updates.clone()).unwrap();
    cache_writer.flush(updates).unwrap();

    let store = CachedStore::from_cache(cache_writer.into_inner(), Some(&mdbx));
    assert_eq!(store.get(&key).unwrap(), Some(b"value".to_vec()));
    assert_eq!(mdbx.get(&key).unwrap(), Some(b"value".to_vec()));
}

#[test]
fn evm_key_and_raw_value_round_trip() {
    use super::super::key::{Key, StoreKeyCodec};

    let directory = tempfile::tempdir().unwrap();
    let mut mdbx: MdbxStore<Key, Vec<u8>, _, _> =
        MdbxStore::open(directory.path(), StoreKeyCodec, IdentityCodec).unwrap();
    let key = Key::Account(alloy_primitives::Address::repeat_byte(0x11));
    let value = vec![42];
    mdbx.flush(vec![(key.clone(), value.clone())]).unwrap();
    assert_eq!(mdbx.get(&key).unwrap(), Some(value));
}
