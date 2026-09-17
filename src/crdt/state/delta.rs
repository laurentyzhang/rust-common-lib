pub enum Delta {
    Bytes(Vec<u8>),
    I64(i64),
    U64Add(u64),
    U64Sub(u64),
    U256Add(alloy_primitives::U256),
    U256Sub(alloy_primitives::U256),
    PathMeta(crate::crdt::path_meta::PathDelta),
    None,
}
