#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Meta {
    pub fully_parallelizable: bool,
    pub is_deferred: bool,
    pub callee: Option<u64>,
}
