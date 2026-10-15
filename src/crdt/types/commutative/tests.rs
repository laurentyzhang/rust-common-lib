use super::Scalar;
use crate::crdt::{Crdt, state::DeltaOp};
use alloy_primitives::U256;

#[test]
fn constructors_validate_bounds() {
    assert!(Scalar::<i64>::new(-10, 10).is_ok());
    assert!(Scalar::<i64>::new(10, -10).is_err());
    assert!(Scalar::<u64>::new(0, 100).is_ok());
    assert!(Scalar::<u64>::new(100, 0).is_err());
    assert!(Scalar::<U256>::new(U256::ZERO, U256::from(100)).is_ok());
    assert!(Scalar::<U256>::new(U256::from(100), U256::ZERO).is_err());
}

#[test]
fn signed_and_unsigned_deltas_share_the_same_operations() {
    let mut signed = Scalar::<i64>::default();
    signed.add_delta(&DeltaOp::Add(10)).unwrap();
    signed.add_delta(&DeltaOp::Sub(13)).unwrap();
    assert_eq!(signed.delta(), Some(&DeltaOp::Sub(3)));

    let mut unsigned = Scalar::<u64>::default();
    unsigned.add_delta(&DeltaOp::Add(10)).unwrap();
    unsigned.add_delta(&DeltaOp::Sub(3)).unwrap();
    assert_eq!(unsigned.delta(), Some(&DeltaOp::Add(7)));
}

#[test]
fn signed_delta_supports_the_full_i64_range() {
    let mut value = Scalar::<i64>::default();
    value.add_delta(&DeltaOp::Sub(1_u64 << 63)).unwrap();
    value.apply_delta();
    assert_eq!(value.value(), Some(&i64::MIN));

    value.add_delta(&DeltaOp::Add(u64::MAX)).unwrap();
    value.apply_delta();
    assert_eq!(value.value(), Some(&i64::MAX));
}
