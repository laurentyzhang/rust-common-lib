use crate::collections;
use crate::crdt::state::Value;
use crate::crdt::state::{Delta, Error};
use crate::store::traits::ReadOnlyStore;

/// A cache for storing all state transitions within a block.
pub struct BlockCache<'a, K> {
    pub(super) cache: collections::shard_map::ShardMap<K, Value>,
    pub(super) fallback: Option<&'a dyn ReadOnlyStore<'a, K, Value>>,
}
