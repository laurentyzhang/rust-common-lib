use super::Scalar;
use crate::crdt::{Crdt, state::DeltaOp};
use alloy_primitives::{I256, U256};

#[test]
fn constructors_validate_bounds() {
    assert!(Scalar::<I256>::new(I256::unchecked_from(-10), I256::unchecked_from(10)).is_ok());
    assert!(Scalar::<I256>::new(I256::unchecked_from(10), I256::unchecked_from(-10)).is_err());
    assert!(Scalar::<U256>::new(U256::ZERO, U256::from(100)).is_ok());
    assert!(Scalar::<U256>::new(U256::from(100), U256::ZERO).is_err());
}

#[test]
fn with_delta_keeps_the_initial_value_until_applied() {
    let delta = DeltaOp::Sub(U256::from(3));
    let mut unsigned = Scalar::with_delta(U256::from(10), delta).unwrap();
    assert_eq!(unsigned.value(), Some(&U256::from(10)));
    assert_eq!(unsigned.delta(), Some(&delta));
    assert_eq!(unsigned.limits(), Some((&U256::ZERO, &U256::MAX)));
    assert_eq!(unsigned.apply_delta().value(), Some(&U256::from(7)));
    assert_eq!(unsigned.delta(), None);

    let mut signed =
        Scalar::with_delta(I256::unchecked_from(-2), DeltaOp::Add(U256::from(5))).unwrap();
    assert_eq!(signed.apply_delta().value(), Some(&I256::unchecked_from(3)));
}

#[test]
fn with_delta_rejects_unapplicable_deltas() {
    let one = U256::from(1);
    assert!(Scalar::with_delta(U256::MAX, DeltaOp::Add(one)).is_err());
    assert!(Scalar::with_delta(U256::ZERO, DeltaOp::Sub(one)).is_err());
    assert!(Scalar::with_delta(I256::MAX, DeltaOp::Add(one)).is_err());
    assert!(Scalar::with_delta(I256::MIN, DeltaOp::Sub(one)).is_err());
}

#[test]
fn signed_and_unsigned_deltas_share_the_same_operations() {
    let mut signed = Scalar::<I256>::default();
    signed.add_delta(&DeltaOp::Add(U256::from(10))).unwrap();
    signed.add_delta(&DeltaOp::Sub(U256::from(13))).unwrap();
    assert_eq!(signed.delta(), Some(&DeltaOp::Sub(U256::from(3))));

    let mut unsigned = Scalar::<U256>::default();
    unsigned.add_delta(&DeltaOp::Add(U256::from(10))).unwrap();
    unsigned.add_delta(&DeltaOp::Sub(U256::from(3))).unwrap();
    assert_eq!(unsigned.delta(), Some(&DeltaOp::Add(U256::from(7))));
}

#[test]
fn signed_delta_supports_the_full_i256_range() {
    let mut value = Scalar::<I256>::default();
    value
        .add_delta(&DeltaOp::Sub(U256::from(1) << 255))
        .unwrap();
    value.apply_delta();
    assert_eq!(value.value(), Some(&I256::MIN));

    value.add_delta(&DeltaOp::Add(U256::MAX)).unwrap();
    value.apply_delta();
    assert_eq!(value.value(), Some(&I256::MAX));
}
