use strum_macros::{EnumIter, FromRepr};

pub mod codec;
pub use codec::NamespaceCodec;

/// Namespace IDs in byte 5 of an ARCO storage key.
#[derive(Clone, Copy, Debug, Eq, PartialEq, EnumIter, FromRepr)]
#[repr(u8)]
pub enum Namespace {
    Io = 0x60,
    Container = 0x84,
    Scalar = 0x85,
    Runtime = 0xa0,
    Multiprocess = 0xb0,
    Conflict = 0xc0,
}
