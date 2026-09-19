use super::{Delta, StateError};
use crate::crdt::Crdt;

#[derive(Clone, PartialEq)]
pub enum Numeric<'a> {
    I64(std::borrow::Cow<'a, crate::crdt::int64::I64>),
    U64(std::borrow::Cow<'a, crate::crdt::uint64::U64>),
    U256(std::borrow::Cow<'a, crate::crdt::u256::U256>),
}

impl<'a> Numeric<'a> {
    pub fn into_owned(self) -> Numeric<'static> {
        match self {
            Self::I64(value) => Numeric::I64(std::borrow::Cow::Owned(value.into_owned())),
            Self::U64(value) => Numeric::U64(std::borrow::Cow::Owned(value.into_owned())),
            Self::U256(value) => Numeric::U256(std::borrow::Cow::Owned(value.into_owned())),
        }
    }

    pub fn from_borrowed(value: &'a Numeric<'_>) -> Self {
        match value {
            Self::I64(value) => Self::I64(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::U64(value) => Self::U64(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::U256(value) => Self::U256(std::borrow::Cow::Borrowed(value.as_ref())),
        }
    }

    pub fn delta(&self) -> Delta {
        match self {
            Self::I64(value) => value
                .delta()
                .map_or(Delta::None, |delta| Delta::I64(*delta)),
            Self::U64(value) => value
                .delta()
                .map_or(Delta::None, |delta| Delta::U64(delta.clone())),
            Self::U256(value) => value
                .delta()
                .map_or(Delta::None, |delta| Delta::U256(delta.clone())),
        }
    }

    pub fn add_delta(&mut self, delta: &Delta) -> Result<(), StateError> {
        match (self, delta) {
            (_, Delta::None) => Ok(()),
            (Self::I64(value), Delta::I64(delta)) => value.to_mut().add_delta(delta).map(|_| ()),
            (Self::U64(value), Delta::U64(delta)) => value.to_mut().add_delta(delta).map(|_| ()),
            (Self::U256(value), Delta::U256(delta)) => value.to_mut().add_delta(delta).map(|_| ()),
            _ => Err(StateError::TypeMismatch),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt::state::{DeltaOp, NumericError, Value};

    macro_rules! unsigned_delta_tests {
        ($module:ident, $crdt:ty, $number:ty, $variant:ident, $error:ident, $accessor:ident, $codec:ident) => {
            mod $module {
                use super::*;
                fn n(value: u64) -> $number {
                    <$number>::from(value)
                }

                #[test]
                fn mixed_deltas_are_deferred_and_cancel_in_both_directions() {
                    let mut value: Value<'static> = <$crdt>::default().into();
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Add(n(10))))
                        .unwrap();
                    value.apply_delta();
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Sub(n(7))))
                        .unwrap();
                    assert_eq!(value.$accessor(), Some(n(10)));
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Add(n(9))))
                        .unwrap();
                    assert_eq!(value.applied().$accessor(), Some(n(12)));
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Sub(n(5))))
                        .unwrap();
                    assert_eq!(value.applied().$accessor(), Some(n(7)));
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Add(n(3))))
                        .unwrap();
                    assert_eq!(value.applied().$accessor(), Some(n(10)));
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Sub(n(10))))
                        .unwrap();
                    value.apply_delta();
                    value.apply_delta();
                    assert_eq!(value.$accessor(), Some(n(0)));
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Sub(n(0))))
                        .unwrap();
                    assert_eq!(value.applied().$accessor(), Some(n(0)));
                }

                #[test]
                fn full_unsigned_range_and_failed_operations_preserve_state() {
                    let mut value: Value<'static> = <$crdt>::default().into();
                    let before = value.clone();
                    assert!(matches!(
                        value.add_delta(&Delta::$variant(DeltaOp::Sub(n(1)))),
                        Err(StateError::$error(NumericError::Underflow(_)))
                    ));
                    assert!(value == before);
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Add(<$number>::MAX)))
                        .unwrap();
                    let before = value.clone();
                    assert!(matches!(
                        value.add_delta(&Delta::$variant(DeltaOp::Add(n(1)))),
                        Err(StateError::$error(NumericError::Overflow(_)))
                    ));
                    assert!(value == before);
                    value.apply_delta();
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Sub(<$number>::MAX)))
                        .unwrap();
                    let before = value.clone();
                    assert!(matches!(
                        value.add_delta(&Delta::$variant(DeltaOp::Sub(n(1)))),
                        Err(StateError::$error(NumericError::Underflow(_)))
                    ));
                    assert!(value == before);
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Add(<$number>::MAX)))
                        .unwrap();
                    assert_eq!(value.applied().$accessor(), Some(<$number>::MAX));
                    value
                        .add_delta(&Delta::$variant(DeltaOp::Sub(<$number>::MAX)))
                        .unwrap();
                    value.apply_delta();
                    assert_eq!(value.$accessor(), Some(n(0)));
                }

                #[test]
                fn limits_are_checked_before_mutating_pending_delta() {
                    type TestCrdt = $crdt;
                    let mut value = TestCrdt {
                        value: n(10),
                        limits: (n(5), n(15)),
                        ..Default::default()
                    };
                    value.add_delta(&DeltaOp::Sub(n(5))).unwrap();
                    let before = value.clone();
                    assert!(matches!(
                        value.add_delta(&DeltaOp::Sub(n(1))),
                        Err(StateError::$error(NumericError::BelowLowerLimit(_)))
                    ));
                    assert!(value == before);
                    value.add_delta(&DeltaOp::Add(n(10))).unwrap();
                    let before = value.clone();
                    assert!(matches!(
                        value.add_delta(&DeltaOp::Add(n(1))),
                        Err(StateError::$error(NumericError::AboveUpperLimit(_)))
                    ));
                    assert!(value == before);
                    value.apply_delta();
                    assert_eq!(value.value(), Some(&n(15)));
                    assert_eq!(value.delta, None);
                }

                #[test]
                fn subtraction_survives_internal_codec_but_storage_omits_pending_delta() {
                    use crate::crdt::codecs::internal::$codec as codec;
                    let mut value = <$crdt>::default();
                    value.add_delta(&DeltaOp::Add(<$number>::MAX)).unwrap();
                    value.apply_delta();
                    let clean = value.clone();
                    value.add_delta(&DeltaOp::Sub(<$number>::MAX)).unwrap();
                    let encoded = codec::encode(&value).unwrap();
                    assert_eq!(encoded[0], 15);
                    let mut decoded = codec::decode(&encoded).unwrap();
                    assert!(decoded == value);
                    decoded.apply_delta();
                    assert_eq!(decoded.value(), Some(&n(0)));
                    assert_eq!(alloy_rlp::encode(&value), alloy_rlp::encode(&clean));
                    let stored =
                        alloy_rlp::decode_exact::<$crdt>(&alloy_rlp::encode(&value)).unwrap();
                    assert!(stored == clean);
                    assert!(codec::decode(&[16]).is_err());
                }
            }
        };
    }

    unsigned_delta_tests!(
        u64_delta,
        crate::crdt::uint64::U64,
        u64,
        U64,
        U64,
        as_u64,
        uint64
    );
    unsigned_delta_tests!(
        u256_delta,
        crate::crdt::u256::U256,
        alloy_primitives::U256,
        U256,
        U256,
        as_u256,
        u256
    );

    #[test]
    fn unsigned_subtraction_rejects_other_numeric_types() {
        let mut u64_value: Value<'static> = crate::crdt::uint64::U64::default().into();
        let mut u256_value: Value<'static> = crate::crdt::u256::U256::default().into();
        assert_eq!(
            u64_value.add_delta(&Delta::U256(DeltaOp::Sub(alloy_primitives::U256::ZERO,))),
            Err(StateError::TypeMismatch)
        );
        assert_eq!(
            u256_value.add_delta(&Delta::U64(DeltaOp::Sub(0))),
            Err(StateError::TypeMismatch)
        );
    }
}
