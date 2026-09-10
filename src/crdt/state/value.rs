use super::{Delta, Error};
use crate::crdt::Crdt;

#[derive(Clone, PartialEq)]
pub enum Value {
    Bytes(crate::crdt::bytes::Bytes),
    I64(crate::crdt::int64::I64),
    U64(crate::crdt::uint64::U64),
    U256(crate::crdt::u256::U256),
    PathMeta(crate::crdt::path_meta::PathMeta),
    None,
}

impl Value {
    pub fn is_numeric(&self) -> bool {
        match self {
            Self::Bytes(value) => value.is_numeric(),
            Self::I64(value) => value.is_numeric(),
            Self::U64(value) => value.is_numeric(),
            Self::U256(value) => value.is_numeric(),
            Self::PathMeta(value) => value.is_numeric(),
            Self::None => false,
        }
    }

    pub fn is_commutative(&self) -> bool {
        match self {
            Self::Bytes(value) => value.is_commutative(),
            Self::I64(value) => value.is_commutative(),
            Self::U64(value) => value.is_commutative(),
            Self::U256(value) => value.is_commutative(),
            Self::PathMeta(value) => value.is_commutative(),
            Self::None => false,
        }
    }

    pub fn add_delta(&mut self, delta: &Delta) -> Result<(), Error> {
        match (self, delta) {
            (_, Delta::None) => Ok(()),
            (Self::Bytes(value), Delta::Bytes(delta)) => value.add_delta(delta).map(|_| ()),
            (Self::I64(value), Delta::I64(delta)) => value.add_delta(delta).map(|_| ()),
            (Self::U64(value), Delta::U64(delta)) => value.add_delta(delta).map(|_| ()),
            (Self::U256(value), Delta::U256(delta)) => value.add_delta(delta).map(|_| ()),
            (Self::PathMeta(value), Delta::PathMeta(delta)) => value.add_delta(delta).map(|_| ()),
            (Self::None, _) => Err(Error::None),
            _ => Err(Error::TypeMismatch),
        }
    }

    pub fn apply_delta(&mut self) -> &Value {
        match self {
            Self::Bytes(value) => {
                value.apply_delta();
                self
            }
            Self::I64(value) => {
                value.apply_delta();
                self
            }
            Self::U64(value) => {
                value.apply_delta();
                self
            }
            Self::U256(value) => {
                value.apply_delta();
                self
            }
            Self::PathMeta(value) => {
                value.apply_delta();
                self
            }
            Self::None => self,
        }
    }
}
