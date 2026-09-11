pub enum Delta {
    Bytes(Vec<u8>),
    I64(i64),
    U64(u64),
    U256(alloy_primitives::U256),
    PathMeta(crate::crdt::path_meta::PathDelta),
    None,
}
