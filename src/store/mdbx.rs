use std::{marker::PhantomData, path::Path};

use reth_db::{
    Database,
    mdbx::{DatabaseArguments, init_db_for},
    transaction::{DbTx, DbTxMut},
};

use super::{FallbackStore, StoreCodec, StoreError};

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

impl<K, V, KC, VC> MdbxStore<K, V, KC, VC>
where
    K: std::hash::Hash + Eq,
    KC: StoreCodec<K>,
    VC: StoreCodec<V>,
{
    pub fn flush(&mut self, updates: Vec<(K, V)>) -> Result<(), StoreError> {
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
#[path = "tests/mdbx.rs"]
mod tests;
