pub enum Delta {
    Bytes(Vec<u8>),
    I64(i64),
    U64(u64),
    U256(alloy_primitives::U256),
    PathMeta(crate::crdt::path_meta::PathDelta),
    None,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    I64((Option<i64>, &'static str)),
    U64((Option<u64>, &'static str)),
    U256((Option<alloy_primitives::U256>, &'static str)),
    None,
    TypeMismatch,
    EntryNotFound,
    ValueAlreadyExists,
}
