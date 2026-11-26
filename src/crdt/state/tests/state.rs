use super::*;
use crate::crdt::Crdt;
use crate::crdt::state::{DeltaOp, NumericError, value::CrdtValue};

macro_rules! unsigned_delta_tests {
    ($module:ident, $crdt:ty, $number:ty, $variant:ident, $error:ident, $accessor:ident, $codec:ident) => {
        mod $module {
            use super::*;
            fn n(value: u64) -> $number {
                <$number>::from(value)
            }

            #[test]
            fn mixed_deltas_are_deferred_and_cancel_in_both_directions() {
                let mut value: CrdtValue<'static> = <$crdt>::default().into();
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Add(n(10))))
                    .unwrap();
                value.apply_delta();
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Sub(n(7))))
                    .unwrap();
                assert_eq!(value.$accessor(), Some(n(10)));
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Add(n(9))))
                    .unwrap();
                assert_eq!(value.applied().$accessor(), Some(n(12)));
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Sub(n(5))))
                    .unwrap();
                assert_eq!(value.applied().$accessor(), Some(n(7)));
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Add(n(3))))
                    .unwrap();
                assert_eq!(value.applied().$accessor(), Some(n(10)));
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Sub(n(10))))
                    .unwrap();
                value.apply_delta();
                value.apply_delta();
                assert_eq!(value.$accessor(), Some(n(0)));
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Sub(n(0))))
                    .unwrap();
                assert_eq!(value.applied().$accessor(), Some(n(0)));
            }

            #[test]
            fn full_unsigned_range_and_failed_operations_preserve_state() {
                let mut value: CrdtValue<'static> = <$crdt>::default().into();
                let before = value.clone();
                assert!(matches!(
                    value.add_delta(&CrdtDelta::$variant(DeltaOp::Sub(n(1)))),
                    Err(CrdtError::$error(NumericError::Underflow(_)))
                ));
                assert!(value == before);
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Add(<$number>::MAX)))
                    .unwrap();
                let before = value.clone();
                assert!(matches!(
                    value.add_delta(&CrdtDelta::$variant(DeltaOp::Add(n(1)))),
                    Err(CrdtError::$error(NumericError::Overflow(_)))
                ));
                assert!(value == before);
                value.apply_delta();
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Sub(<$number>::MAX)))
                    .unwrap();
                let before = value.clone();
                assert!(matches!(
                    value.add_delta(&CrdtDelta::$variant(DeltaOp::Sub(n(1)))),
                    Err(CrdtError::$error(NumericError::Underflow(_)))
                ));
                assert!(value == before);
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Add(<$number>::MAX)))
                    .unwrap();
                assert_eq!(value.applied().$accessor(), Some(<$number>::MAX));
                value
                    .add_delta(&CrdtDelta::$variant(DeltaOp::Sub(<$number>::MAX)))
                    .unwrap();
                value.apply_delta();
                assert_eq!(value.$accessor(), Some(n(0)));
            }

            #[test]
            fn limits_are_checked_before_mutating_pending_delta() {
                type TestCrdt = $crdt;
                let mut value = TestCrdt {
                    value: n(10),
                    min: n(5),
                    max: n(15),
                    ..Default::default()
                };
                value.add_delta(&DeltaOp::Sub(n(5))).unwrap();
                let before = value.clone();
                assert!(matches!(
                    value.add_delta(&DeltaOp::Sub(n(1))),
                    Err(CrdtError::$error(NumericError::BelowLowerLimit(_)))
                ));
                assert!(value == before);
                value.add_delta(&DeltaOp::Add(n(10))).unwrap();
                let before = value.clone();
                assert!(matches!(
                    value.add_delta(&DeltaOp::Add(n(1))),
                    Err(CrdtError::$error(NumericError::AboveUpperLimit(_)))
                ));
                assert!(value == before);
                value.apply_delta();
                assert_eq!(value.value(), Some(&n(15)));
                assert_eq!(value.delta, None);
            }
        }
    };
}

unsigned_delta_tests!(
    u256_delta,
    crate::crdt::Scalar<alloy_primitives::U256>,
    alloy_primitives::U256,
    U256,
    U256,
    as_u256,
    u256
);

#[test]
fn numeric_deltas_reject_other_numeric_types() {
    let mut i256_value: CrdtValue<'static> =
        crate::crdt::Scalar::<alloy_primitives::I256>::default().into();
    let mut u256_value: CrdtValue<'static> =
        crate::crdt::Scalar::<alloy_primitives::U256>::default().into();
    assert_eq!(
        i256_value.add_delta(&CrdtDelta::U256(
            DeltaOp::Sub(alloy_primitives::U256::ZERO,)
        )),
        Err(CrdtError::TypeMismatch)
    );
    assert_eq!(
        u256_value.add_delta(&CrdtDelta::I256(
            DeltaOp::Sub(alloy_primitives::U256::ZERO,)
        )),
        Err(CrdtError::TypeMismatch)
    );
}

mod tracked {
    use super::Tracked;
    use crate::crdt::{
        Scalar,
        state::{CrdtDelta, value::CrdtValue},
    };

    #[test]
    fn deleting_created_value_returns_it_to_missing() {
        let mut tracked = Tracked::new_owned(Scalar::<alloy_primitives::U256>::default().into(), 7);
        let _ = tracked.get();
        tracked.check();
        assert!(tracked.add_delta(CrdtDelta::None).is_ok());
        assert!(tracked.remove().is_ok());
        assert!(tracked.is_missing());
        assert!(matches!(tracked.current(), CrdtValue::None));

        assert_eq!(tracked.operations.reads.count(), 1);
        assert_eq!(tracked.operations.existence_checks.count(), 1);
        assert_eq!(tracked.operations.deltas.count(), 1);
    }

    #[test]
    fn deleting_preexisting_value_is_permanent() {
        let value: CrdtValue<'static> = CrdtValue::U256(std::borrow::Cow::Owned(Scalar::<
            alloy_primitives::U256,
        >::default(
        )));
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
        assert!(matches!(empty.original(), CrdtValue::None));
        assert!(matches!(empty.current(), CrdtValue::None));

        let owned = Tracked::new_owned(Scalar::<alloy_primitives::U256>::default().into(), 11);
        assert_eq!(owned.id, 11);
        assert!(owned.is_created());
        assert!(matches!(owned.original(), CrdtValue::None));

        let value: CrdtValue<'static> = Scalar::<alloy_primitives::U256>::default().into();
        let borrowed = Tracked::new_borrowed(&value, 12);
        assert_eq!(borrowed.id, 12);
        assert!(!borrowed.is_created());
        assert!(borrowed.is_preexisting());
        assert!(borrowed.original() == &value);
        assert!(borrowed.current() == &value);
    }

    #[test]
    fn only_mutations_make_a_record_non_read_only() {
        let value: CrdtValue<'static> = Scalar::<alloy_primitives::U256>::default().into();
        let mut tracked = Tracked::new_borrowed(&value, 7);
        assert!(tracked.is_read_only());

        assert!(tracked.get().is_some());
        assert!(tracked.is_read_only());

        tracked.check();
        assert!(tracked.is_read_only());

        assert_eq!(
            tracked.set(Scalar::<alloy_primitives::U256>::default().into()),
            Err(crate::store::traits::StoreError::ValueCannotBeRecreated)
        );
        assert!(!tracked.is_read_only());
    }

    #[test]
    fn ownership_conversions_preserve_id() {
        let value: CrdtValue<'static> = Scalar::<alloy_primitives::U256>::default().into();
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
    use crate::crdt::state::{Status, Tracked, status::Tag, value::ValueSnapshots};

    fn status_record(original: Status, current: Status) -> Tracked<Status, Status> {
        Tracked {
            id: 0,
            value: ValueSnapshots { original, current },
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

mod u256_set {
    use crate::crdt::state::{CrdtDelta, CrdtError, value::CrdtValue};
    use crate::crdt::types::collections::SetDelta;
    use crate::crdt::{Crdt, Set};
    use alloy_primitives::U256;
    use std::borrow::Cow;

    /// Builds a U256-set delta from additions in order and removals.
    fn delta(added: &[u64], removed: &[u64]) -> CrdtDelta {
        let mut delta = SetDelta::default();
        for element in added {
            delta.added.insert(U256::from(*element));
        }
        delta.removed = removed.iter().map(|e| U256::from(*e)).collect();
        CrdtDelta::U256Set(delta)
    }

    /// Returns the set inside a U256-set value.
    fn set<'a>(value: &'a CrdtValue<'_>) -> &'a Set<U256> {
        match value {
            CrdtValue::U256Set(set) => set,
            _ => panic!("not a U256 set"),
        }
    }

    #[test]
    fn add_delta_reports_changes_and_apply_delta_commits() {
        let mut value = CrdtValue::U256Set(Cow::Owned(Set::default()));

        assert_eq!(value.add_delta(&delta(&[1, 2], &[])), Ok(true));
        assert_eq!(value.add_delta(&delta(&[1], &[])), Ok(false));
        assert!(value.has_delta());
        assert_eq!(value.delta(), delta(&[1, 2], &[]));

        value.apply_delta();
        assert!(!value.has_delta());
        assert_eq!(value.delta(), CrdtDelta::None);
        assert_eq!(set(&value).get_at(0), Some(&U256::from(1)));
        assert_eq!(set(&value).get_at(1), Some(&U256::from(2)));

        // Removing a committed element compacts the set at commit.
        assert_eq!(value.add_delta(&delta(&[], &[1])), Ok(true));
        value.apply_delta();
        assert!(!set(&value).contains(&U256::from(1)));
        assert_eq!(set(&value).get_at(0), Some(&U256::from(2)));
        assert_eq!(set(&value).value().unwrap().span(), 1);
    }

    #[test]
    fn none_delta_is_a_noop() {
        let mut value = CrdtValue::U256Set(Cow::Owned(Set::default()));

        assert_eq!(value.add_delta(&CrdtDelta::None), Ok(false));
        assert!(!value.has_delta());
    }

    #[test]
    fn rejects_other_delta_types() {
        let mut u256_set = CrdtValue::U256Set(Cow::Owned(Set::default()));
        let mut u64_set: CrdtValue<'static> = Set::<u64>::default().into();

        assert_eq!(
            u256_set.add_delta(&CrdtDelta::U64Set(SetDelta::default())),
            Err(CrdtError::TypeMismatch)
        );
        assert_eq!(
            u64_set.add_delta(&delta(&[1], &[])),
            Err(CrdtError::TypeMismatch)
        );
    }

    #[test]
    fn borrowed_and_owned_copies_are_equal() {
        let mut value = CrdtValue::U256Set(Cow::Owned(Set::default()));
        value.add_delta(&delta(&[1], &[])).unwrap();

        let borrowed = CrdtValue::from_borrowed(&value);
        assert!(matches!(&borrowed, CrdtValue::U256Set(Cow::Borrowed(_))));
        assert_eq!(borrowed, value);
        assert_eq!(borrowed.into_owned(), value);
    }
}
