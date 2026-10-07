use super::*;
use crate::crdt::Crdt;
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
                let stored = alloy_rlp::decode_exact::<$crdt>(&alloy_rlp::encode(&value)).unwrap();
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

mod tracked {
    use super::Tracked;
    use crate::crdt::{
        state::{Delta, Numeric, Value},
        uint64::U64,
    };

    #[test]
    fn deleting_created_value_returns_it_to_missing() {
        let mut tracked = Tracked::new_owned(U64::default().into(), 7);
        let _ = tracked.get();
        tracked.check();
        assert!(tracked.add_delta(Delta::None).is_ok());
        assert!(tracked.remove().is_ok());
        assert!(tracked.is_missing());
        assert!(matches!(tracked.current(), Value::None));

        assert_eq!(tracked.operations.reads.count(), 1);
        assert_eq!(tracked.operations.existence_checks.count(), 1);
        assert_eq!(tracked.operations.deltas.count(), 1);
    }

    #[test]
    fn deleting_preexisting_value_is_permanent() {
        let value: Value<'static> =
            Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));
        let mut tracked = Tracked::new_borrowed(&value, 7);

        assert!(tracked.remove().is_ok());
        assert!(tracked.is_deleted());
        assert_eq!(
            tracked.set(value.clone()),
            Err(crate::store::traits::StoreError::ValueCannotBeRecreated)
        );
    }

    #[test]
    fn constructors_record_id_and_origin() {
        let empty = Tracked::new_owned_empty(10);
        assert!(matches!(empty.original(), Value::None));
        assert!(matches!(empty.current(), Value::None));

        let owned = Tracked::new_owned(U64::default().into(), 11);
        assert_eq!(owned.id, 11);
        assert!(owned.is_created());
        assert!(matches!(owned.original(), Value::None));

        let value: Value<'static> = U64::default().into();
        let borrowed = Tracked::new_borrowed(&value, 12);
        assert_eq!(borrowed.id, 12);
        assert!(!borrowed.is_created());
        assert!(borrowed.is_preexisting());
        assert!(borrowed.original() == &value);
        assert!(borrowed.current() == &value);
    }

    #[test]
    fn only_mutations_make_a_record_non_read_only() {
        let value: Value<'static> = U64::default().into();
        let mut tracked = Tracked::new_borrowed(&value, 7);
        assert!(tracked.is_read_only());

        assert!(tracked.get().is_some());
        assert!(tracked.is_read_only());

        tracked.check();
        assert!(tracked.is_read_only());

        assert_eq!(
            tracked.set(U64::default().into()),
            Err(crate::store::traits::StoreError::ValueCannotBeRecreated)
        );
        assert!(!tracked.is_read_only());
    }

    #[test]
    fn ownership_conversions_preserve_id() {
        let value: Value<'static> = U64::default().into();
        let tracked = Tracked::new_borrowed(&value, 42);
        let cloned = tracked.owned_clone();

        assert_eq!(cloned.id, 42);
        assert!(cloned.original() == &value);

        let owned = tracked.into_owned();
        assert_eq!(owned.id, 42);
        assert!(owned.original() == &value);
    }
}

mod tracked_creation {
    use super::*;
    use crate::crdt::state::{Status, Tracked, status::Tag, value::Values};

    fn status_record(original: Status, current: Status) -> Tracked<Status, Status> {
        Tracked {
            id: 0,
            value: Values { original, current },
            operations: Default::default(),
        }
    }

    #[test]
    fn missing_to_present_is_newly_created() {
        let record = status_record(Status::Tag(Tag::Missing), Status::Tag(Tag::Default));

        assert!(record.is_newly_created());
    }

    #[test]
    fn missing_to_missing_is_not_newly_created() {
        let record = status_record(Status::Tag(Tag::Missing), Status::Tag(Tag::Missing));

        assert!(!record.is_newly_created());
    }

    #[test]
    fn preexisting_value_and_deletion_are_not_newly_created() {
        let preexisting = status_record(Status::Tag(Tag::Stripped), Status::Tag(Tag::Stripped));
        let deleted = status_record(Status::Tag(Tag::Stripped), Status::Tag(Tag::Deleted));

        assert!(!preexisting.is_newly_created());
        assert!(!deleted.is_newly_created());
    }
}
