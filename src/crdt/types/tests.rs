use super::{Bytes, I64, PathDelta, PathMeta, U64, U256};
use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::{NumericError, StateError};
use alloy_primitives::U256 as AlloyU256;

#[test]
fn default_has_no_value_or_delta() {
    let bytes = Bytes::default();

    assert_eq!(bytes.value(), None);
    assert_eq!(bytes.delta.as_deref(), None);
    assert_eq!(bytes.limits(), None);
    assert!(!bytes.is_numeric());
    assert!(!bytes.is_commutative());
}

#[test]
fn delta_is_the_current_value() {
    let mut bytes = Bytes::default();

    bytes.add_delta(&[1, 2, 3]).expect("add delta");

    assert_eq!(bytes.value(), Some(&[1, 2, 3][..]));
    assert_eq!(bytes.delta.as_deref(), Some(&[1, 2, 3][..]));
}

#[test]
fn setting_delta_again_replaces_the_value() {
    let mut bytes = Bytes::default();
    bytes.add_delta(&[1, 2, 3]).expect("add delta");

    bytes.add_delta(&[4, 5]).expect("replace delta");

    assert_eq!(bytes.value(), Some(&[4, 5][..]));
    assert_eq!(bytes.delta.as_deref(), Some(&[4, 5][..]));
}

#[test]
fn apply_delta_is_a_no_op() {
    let mut bytes = Bytes::default();
    bytes.add_delta(&[1, 2, 3]).expect("add delta");

    assert_eq!(bytes.apply_delta().value(), Some(&[1, 2, 3][..]));
    assert_eq!(bytes.delta.as_deref(), Some(&[1, 2, 3][..]));
}

#[test]
fn empty_delta_is_preserved() {
    let mut bytes = Bytes::default();
    bytes.add_delta(&[]).expect("add empty delta");

    assert_eq!(bytes.value(), Some(&[][..]));
    assert_eq!(bytes.delta.as_deref(), Some(&[][..]));
}

#[test]
fn cache_weight_counts_struct_and_payload_bytes() {
    let mut bytes = Bytes::default();
    let base = std::mem::size_of::<Bytes>();
    assert_eq!(bytes.cache_weight(), base);

    bytes.add_delta(&[1, 2, 3]).expect("add delta");
    assert_eq!(bytes.cache_weight(), base + 3);

    bytes.add_delta(&[4, 5]).expect("replace delta");
    assert_eq!(bytes.cache_weight(), base + 2);
}

#[test]
fn i64_constructor_validates_bounds_before_initial_value() {
    assert!(I64::new(-10, 10).is_ok());
    assert!(matches!(
        I64::new(10, -10),
        Err(StateError::I64(NumericError::InvalidLimits(_)))
    ));
}

#[test]
fn u64_constructor_uses_lower_then_upper() {
    assert!(U64::new(0, 100).is_ok());
    assert!(U64::new(100, 0).is_err());
}

#[test]
fn u256_constructor_uses_lower_then_upper() {
    assert!(U256::new(AlloyU256::ZERO, AlloyU256::from(100)).is_ok());
    assert!(U256::new(AlloyU256::from(100), AlloyU256::ZERO).is_err());
}

#[test]
fn path_default_is_empty() {
    let path = PathMeta::default();

    assert_eq!(path.value().unwrap().len(), 0);
    assert_eq!(path.delta.as_ref(), None);
    assert!(!path.is_numeric());
    assert!(path.is_commutative());
}

#[test]
fn path_add_delta_does_not_change_value_until_applied() {
    let mut path = PathMeta::default();
    let delta = PathDelta {
        added: vec![10, 20],
        removed: vec![],
    };

    path.add_delta(&delta).unwrap();

    assert_eq!(path.value().unwrap().get(&10), None);
    assert_eq!(path.delta.as_ref(), Some(&delta));
}

#[test]
fn path_apply_delta_adds_and_removes_entries() {
    let mut path = PathMeta::default();
    path.add_delta(&PathDelta {
        added: vec![10, 20],
        removed: vec![],
    })
    .unwrap();
    path.apply_delta();

    path.add_delta(&PathDelta {
        added: vec![30],
        removed: vec![10],
    })
    .unwrap();
    path.apply_delta();

    let entries = path.value().unwrap();
    assert_eq!(entries.get(&10), None);
    assert_eq!(entries.get(&20), Some(&20));
    assert_eq!(entries.get(&30), Some(&30));
    assert_eq!(path.delta.as_ref(), None);
}

#[test]
fn path_add_delta_accumulates_unique_entries() {
    let mut path = PathMeta::default();
    path.add_delta(&PathDelta {
        added: vec![10, 20],
        removed: vec![30],
    })
    .unwrap();
    path.add_delta(&PathDelta {
        added: vec![20, 30],
        removed: vec![30, 40],
    })
    .unwrap();

    assert_eq!(
        path.delta.as_ref(),
        Some(&PathDelta {
            added: vec![10, 20, 30],
            removed: vec![30, 40],
        })
    );

    path.apply_delta();
    let entries = path.value().unwrap();
    assert_eq!(entries.get(&10), Some(&10));
    assert_eq!(entries.get(&20), Some(&20));
    assert_eq!(entries.get(&30), Some(&30));
    assert_eq!(entries.get(&40), None);
}
