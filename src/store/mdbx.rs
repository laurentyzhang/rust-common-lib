use std::{marker::PhantomData, path::Path};

use reth_db::{
    Database,
    mdbx::{DatabaseArguments, init_db_for},
    transaction::{DbTx, DbTxMut},
};

use super::{FallbackStore, StoreCodec, StoreError};
use crate::committer::traits::TransitionWriter;

mod tables {
    use reth_db::{TableSet, TableType, TableViewer, table::TableInfo, tables};
    use std::fmt;

    tables! {
        table CachedState {
            type Key = Vec<u8>;
            type Value = Vec<u8>;
        }
    }
}

pub struct MdbxStore<K, V, KC, VC> {
    db: reth_db::DatabaseEnv,
    key_codec: KC,
    value_codec: VC,
    marker: PhantomData<fn(K, V)>,
}

impl<K, V, KC, VC> MdbxStore<K, V, KC, VC> {
    pub fn open(
        path: impl AsRef<Path>,
        key_codec: KC,
        value_codec: VC,
    ) -> Result<Self, StoreError> {
        let db = init_db_for::<_, tables::Tables>(path, DatabaseArguments::default())
            .map_err(|error| StoreError::Database(error.to_string()))?;
        Ok(Self {
            db,
            key_codec,
            value_codec,
            marker: PhantomData,
        })
    }
}

impl<K, V, KC, VC> FallbackStore<K, V> for MdbxStore<K, V, KC, VC>
where
    KC: StoreCodec<K>,
    VC: StoreCodec<V>,
{
    fn get(&self, key: &K) -> Result<Option<V>, StoreError> {
        let key = self.key_codec.encode(key)?;
        let tx = self.db.tx().map_err(database_error)?;
        let encoded = tx.get::<tables::CachedState>(key).map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        encoded
            .as_deref()
            .map(|value| self.value_codec.decode(value))
            .transpose()
    }
}

impl<K, V, KC, VC> TransitionWriter<K, V> for MdbxStore<K, V, KC, VC>
where
    K: std::hash::Hash + Eq,
    KC: StoreCodec<K>,
    VC: StoreCodec<V>,
{
    fn flush(&mut self, updates: Vec<(K, V)>) -> Result<(), StoreError> {
        let encoded = updates
            .iter()
            .map(|(key, value)| Ok((self.key_codec.encode(key)?, self.value_codec.encode(value)?)))
            .collect::<Result<Vec<_>, StoreError>>()?;

        let tx = self.db.tx_mut().map_err(database_error)?;
        for (key, value) in encoded {
            tx.put::<tables::CachedState>(key, value)
                .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    }
}

fn database_error(error: reth_db::DatabaseError) -> StoreError {
    StoreError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        committer::traits::TransitionWriter,
        store::{IdentityCodec, cache::CachedStore},
    };

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
        use crate::{committer::Committer, store::CachedStoreWriter};

        let directory = tempfile::tempdir().unwrap();
        let mut mdbx = MdbxStore::open(directory.path(), IdentityCodec, IdentityCodec).unwrap();
        let key = b"key".to_vec();

        let store = CachedStore::new(4, Some(&mdbx));
        assert_eq!(store.get(&key).unwrap(), None);
        let mut cache_writer = CachedStoreWriter::new(store.into_cache());

        {
            let mut committer = Committer::new();
            committer.register(&mut mdbx);
            committer.register(&mut cache_writer);
            committer
                .buffer(1, vec![(key.clone(), b"value".to_vec())])
                .unwrap();
            committer.stage(vec![1]).unwrap();
            committer.flush().unwrap();
        }

        let store = CachedStore::from_cache(cache_writer.into_inner(), Some(&mdbx));
        assert_eq!(store.get(&key).unwrap(), Some(b"value".to_vec()));
        assert_eq!(mdbx.get(&key).unwrap(), Some(b"value".to_vec()));
    }

    #[test]
    fn evm_key_and_crdt_value_round_trip() {
        use crate::{
            crdt::{
                state::{CrdtValue, Numeric},
                uint64::U64,
            },
            evm::{Key, KeyCodec},
            store::CrdtValueCodec,
        };

        let directory = tempfile::tempdir().unwrap();
        let mut mdbx: MdbxStore<Key, CrdtValue<'static>, _, _> =
            MdbxStore::open(directory.path(), KeyCodec, CrdtValueCodec).unwrap();
        let key = Key::Account(alloy_primitives::Address::repeat_byte(0x11));
        let value = CrdtValue::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64 {
            value: 42,
            ..U64::default()
        })));

        mdbx.flush(vec![(key.clone(), value.clone())]).unwrap();
        assert_eq!(mdbx.get(&key).unwrap(), Some(value));
    }
}
