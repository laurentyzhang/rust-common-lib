use strum_macros::{EnumIter, FromRepr};

pub mod codec;

pub use codec::NamespaceCodec;

/// Namespace IDs in byte 5 of an ARCO storage key.
#[derive(Clone, Copy, Debug, Eq, PartialEq, EnumIter, FromRepr)]
#[repr(u8)]
pub enum Namespace {
    Set = 0x84,
    U256 = 0x85,
    I256 = 0x86,
    Runtime = 0xa0,
    Multiprocess = 0xb0,
    Meta = 0xc0,
}
