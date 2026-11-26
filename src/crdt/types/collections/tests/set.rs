use super::{Set, SetDelta};
use crate::crdt::crdt::Crdt;
use std::cmp::Ordering;
use std::hash::Hash;

/// Builds a delta from additions in order and removals.
fn delta<T: Clone + Eq + Hash + Ord>(added: &[T], removed: &[T]) -> SetDelta<T> {
    let mut delta = SetDelta::default();
    for element in added {
        delta.added.insert(element.clone());
    }
    delta.removed = removed.iter().cloned().collect();
    delta
}

#[test]
fn default_is_empty() {
    let set = Set::<u64>::default();

    assert_eq!(set.value().unwrap().len(), 0);
    assert_eq!(set.delta.as_ref(), None);
    assert!(!set.is_numeric());
    assert!(set.is_commutative());
}

#[test]
fn add_delta_does_not_change_value_until_applied() {
    let mut set = Set::default();
    let pending = delta(&[10, 20], &[]);

    set.add_delta(&pending).unwrap();

    assert_eq!(set.value().unwrap().get(&10), None);
    assert_eq!(set.delta.as_ref(), Some(&pending));
}

#[test]
fn supports_fixed_length_byte_arrays() {
    let mut set = Set::<[u8; 32]>::default();
    let value = [7; 32];

    set.add_delta(&delta(&[value], &[])).unwrap();
    set.apply_delta();

    assert_eq!(set.value().unwrap().get(&value), Some(&value));
}

#[test]
fn apply_delta_adds_and_removes_entries() {
    let mut set = Set::default();
    set.add_delta(&delta(&[10, 20], &[])).unwrap();
    set.apply_delta();

    set.add_delta(&delta(&[30], &[10])).unwrap();
    set.apply_delta();

    let entries = set.value().unwrap();
    assert_eq!(entries.get(&10), None);
    assert_eq!(entries.get(&20), Some(&20));
    assert_eq!(entries.get(&30), Some(&30));
    assert_eq!(set.delta.as_ref(), None);
}

#[test]
fn add_delta_accumulates_unique_entries() {
    let mut set = Set::default();
    set.add_delta(&delta(&[10, 20], &[30])).unwrap();
    set.add_delta(&delta(&[20, 30], &[30, 40])).unwrap();

    // Additions keep first-insertion order; duplicates are merged.
    // Removing pending 30 leaves a hole; uncommitted 30 and 40 are not staged for removal.
    let pending = set.delta.as_ref().unwrap();
    assert_eq!(pending.added.slots(), &[Some(10), Some(20), None]);
    assert!(pending.removed.is_empty());

    set.apply_delta();
    let entries = set.value().unwrap();
    assert_eq!(entries.get(&10), Some(&10));
    assert_eq!(entries.get(&20), Some(&20));
    assert_eq!(entries.get(&30), None);
    assert_eq!(entries.get(&40), None);
}

#[test]
fn delta_equality_depends_on_addition_order() {
    assert_eq!(delta(&[1, 2], &[3, 4]), delta(&[1, 2], &[4, 3]));
    assert_ne!(delta(&[1, 2], &[]), delta(&[2, 1], &[]));
}

#[test]
fn compare_uses_operation_count_and_places_none_last() {
    let mut one_operation = Set::default();
    one_operation.add_delta(&delta(&[1], &[])).unwrap();

    let mut two_operations = Set::default();
    two_operations.add_delta(&delta(&[1, 2], &[])).unwrap();

    let no_delta = Set::default();

    assert_eq!(one_operation.compare(&two_operations), Ordering::Less);
    assert_eq!(two_operations.compare(&one_operation), Ordering::Greater);
    assert_eq!(one_operation.compare(&one_operation), Ordering::Equal);
    assert_eq!(one_operation.compare(&no_delta), Ordering::Less);
    assert_eq!(no_delta.compare(&one_operation), Ordering::Greater);
    assert_eq!(no_delta.compare(&no_delta), Ordering::Equal);
}

/// Builds a set whose committed entries are `elements`, in order.
fn committed(elements: &[u64]) -> Set<u64> {
    let mut set = Set::default();
    set.add_delta(&delta(elements, &[])).unwrap();
    set.apply_delta();
    set
}

#[test]
fn adding_committed_entry_only_cancels_its_pending_removal() {
    let mut set = committed(&[1]);

    // A no-op does not create a delta.
    assert!(!set.add_delta(&delta(&[1], &[])).unwrap());
    assert_eq!(set.delta.as_ref(), None);

    assert!(set.add_delta(&delta(&[], &[1])).unwrap());
    assert_eq!(set.delta.as_ref(), Some(&delta(&[], &[1])));

    assert!(set.add_delta(&delta(&[1], &[])).unwrap());
    assert_eq!(set.delta.as_ref(), Some(&SetDelta::default()));
}

#[test]
fn removing_pending_addition_leaves_hole_and_absent_removal_is_noop() {
    let mut set = Set::<u64>::default();

    assert!(set.add_delta(&delta(&[5], &[])).unwrap());
    assert!(!set.add_delta(&delta(&[5], &[])).unwrap());
    assert!(set.add_delta(&delta(&[], &[5])).unwrap());
    assert!(!set.add_delta(&delta(&[], &[6])).unwrap());

    let pending = set.delta.as_ref().unwrap();
    assert_eq!(pending.added.slots(), &[None]);
    assert!(pending.removed.is_empty());

    // Re-adding a cancelled element appends it at a new position.
    assert!(set.add_delta(&delta(&[5], &[])).unwrap());
    assert_eq!(set.delta.as_ref().unwrap().added.slots(), &[None, Some(5)]);
}

#[test]
fn apply_delta_compacts_committed_entries() {
    let mut set = committed(&[1, 2, 3]);
    set.add_delta(&delta(&[4], &[2])).unwrap();
    set.apply_delta();

    let entries = set.value().unwrap();
    assert_eq!(entries.span(), 3);
    assert_eq!(entries.get_at(0), Some(&1));
    assert_eq!(entries.get_at(1), Some(&3));
    assert_eq!(entries.get_at(2), Some(&4));
}

#[test]
fn get_at_reads_committed_then_pending_with_holes() {
    let mut set = committed(&[1, 2]);
    set.add_delta(&delta(&[3, 4], &[1])).unwrap();
    set.add_delta(&delta(&[], &[3])).unwrap();

    assert_eq!(set.span(), 4);
    assert_eq!(set.len(), 2);
    assert_eq!(set.get_at(0), None); // Pending removal.
    assert_eq!(set.get_at(1), Some(&2));
    assert_eq!(set.get_at(2), None); // Cancelled addition.
    assert_eq!(set.get_at(3), Some(&4));
    assert_eq!(set.get_at(4), None);

    assert!(!set.contains(&1)); // Pending removal.
    assert!(set.contains(&2));
    assert!(!set.contains(&3)); // Cancelled addition.
    assert!(set.contains(&4));
    assert!(!set.contains(&5));
}
