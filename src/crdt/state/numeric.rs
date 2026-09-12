use super::{Delta, Error};
use crate::crdt::Crdt;

#[derive(Clone, PartialEq)]
pub enum Numeric<'a> {
    I64(std::borrow::Cow<'a, crate::crdt::int64::I64>),
    U64(std::borrow::Cow<'a, crate::crdt::uint64::U64>),
    U256(std::borrow::Cow<'a, crate::crdt::u256::U256>),
}

impl<'a> Numeric<'a> {
    pub fn borrowed(value: &'a Numeric<'_>) -> Self {
        match value {
            Self::I64(value) => Self::I64(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::U64(value) => Self::U64(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::U256(value) => Self::U256(std::borrow::Cow::Borrowed(value.as_ref())),
        }
    }

    pub fn add_delta(&mut self, delta: &Delta) -> Result<(), Error> {
        match (self, delta) {
            (_, Delta::None) => Ok(()),
            (Self::I64(value), Delta::I64(delta)) => value.to_mut().add_delta(delta).map(|_| ()),
            (Self::U64(value), Delta::U64(delta)) => value.to_mut().add_delta(delta).map(|_| ()),
            (Self::U256(value), Delta::U256(delta)) => value.to_mut().add_delta(delta).map(|_| ()),
            _ => Err(Error::TypeMismatch),
        }
    }

    pub fn apply_delta(&mut self) -> &Self {
        match self {
            Self::I64(value) => {
                value.to_mut().apply_delta();
            }
            Self::U64(value) => {
                value.to_mut().apply_delta();
            }
            Self::U256(value) => {
                value.to_mut().apply_delta();
            }
        };
        self
    }
}
