use strum_macros::FromRepr;

use crate::namespace::Namespace;

/// Namespaces supported by scalar storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq, FromRepr)]
#[repr(u8)]
pub enum ScalarKind {
    U256 = Namespace::U256 as u8,
    I256 = Namespace::I256 as u8,
}

impl TryFrom<Namespace> for ScalarKind {
    type Error = ();

    fn try_from(namespace: Namespace) -> Result<Self, Self::Error> {
        Self::from_repr(namespace as u8).ok_or(())
    }
}

impl From<ScalarKind> for Namespace {
    fn from(kind: ScalarKind) -> Self {
        match kind {
            ScalarKind::U256 => Self::U256,
            ScalarKind::I256 => Self::I256,
        }
    }
}
