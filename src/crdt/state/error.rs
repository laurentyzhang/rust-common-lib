#[derive(Debug, PartialEq)]
pub enum Error {
    I64(&'static str),
    U64(&'static str),
    U256(&'static str),
    None,
    TypeMismatch,
    EntryNotFound,
    ValueCannotBeRecreated,
}
