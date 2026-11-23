//! Fixed encoded widths, for values stored as sequence elements.

/// A value whose encoding is always `N` bytes, usable as a sequence element.
pub trait Width<const N: usize> {}

impl Width<8> for u64 {}

impl Width<32> for alloy_primitives::U256 {}
