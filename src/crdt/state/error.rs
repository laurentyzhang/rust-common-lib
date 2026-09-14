#[derive(Debug, PartialEq)]
pub enum StateError {
    I64(&'static str),
    U64(&'static str),
    U256(&'static str),
    None,
    TypeMismatch,
}
