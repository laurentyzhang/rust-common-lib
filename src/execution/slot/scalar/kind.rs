use strum_macros::FromRepr;

use crate::namespace::Namespace;

/// Namespaces supported by scalar storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq, FromRepr)]
#[repr(u8)]
pub enum Kind {
    U256 = Namespace::U256 as u8,
    I256 = Namespace::I256 as u8,
}

impl TryFrom<Namespace> for Kind {
    type Error = ();

    fn try_from(namespace: Namespace) -> Result<Self, Self::Error> {
        Self::from_repr(namespace as u8).ok_or(())
    }
}

impl From<Kind> for Namespace {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::U256 => Self::U256,
            Kind::I256 => Self::I256,
        }
    }
}
