#[derive(Clone, Debug, PartialEq)]
pub enum Delta {
    Bytes(Vec<u8>),
    I64(i64),
    U64(DeltaOp<u64>),
    U256(DeltaOp<alloy_primitives::U256>),
    U64Set(Vec<DeltaOp<u64>>),
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeltaOp<T> {
    Add(T),
    Sub(T),
}
