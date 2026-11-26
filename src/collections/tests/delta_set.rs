use super::DeltaSet;

fn key(value: &str) -> String {
    value.to_owned()
}

#[test]
fn insert_and_get_by_key_and_index() {
    let mut set = DeltaSet::new(4);

    assert_eq!(set.insert(key("alpha")), Some(0));
    assert_eq!(set.insert(key("beta")), Some(1));

    assert_eq!(set.get(&key("alpha")).map(String::as_str), Some("alpha"));
    assert_eq!(set.get_at(1).map(String::as_str), Some("beta"));
    assert_eq!(set.get_at(2), None);
}

#[test]
fn duplicate_insert_is_rejected() {
    let mut set = DeltaSet::new(0);

    assert_eq!(set.insert(key("alpha")), Some(0));
    assert_eq!(set.insert(key("alpha")), None);
    assert_eq!(set.len(), 1);
}

#[test]
fn remove_by_key_and_index_clears_elements() {
    let mut set = DeltaSet::new(0);
    set.insert(key("alpha"));
    set.insert(key("beta"));

    assert_eq!(set.remove(&key("alpha")), Some((key("alpha"), 0)));
    assert_eq!(set.get(&key("alpha")), None);
    assert_eq!(set.get_at(0), None);

    assert_eq!(set.remove_at(1), Some((key("beta"), 1)));
    assert_eq!(set.get(&key("beta")), None);
    assert_eq!(set.get_at(1), None);
}

#[test]
fn diff_reports_added_and_removed_keys() {
    let mut set = DeltaSet::new(0);
    set.insert(key("alpha"));
    set.insert(key("beta"));
    set.committed = set.keys.clone();

    set.remove(&key("alpha"));
    set.insert(key("gamma"));

    let (mut added, mut removed) = set.diff();
    added.sort();
    removed.sort();

    assert_eq!(added, vec![key("gamma")]);
    assert_eq!(removed, vec![key("alpha")]);
}

#[test]
fn apply_delta_keeps_elements_and_keys_consistent() {
    let mut set = DeltaSet::new(0);
    set.insert(key("alpha"));
    set.insert(key("beta"));

    set.apply_delta(&[key("alpha")], &[key("gamma")]);

    assert_eq!(set.get(&key("alpha")), None);
    assert_eq!(set.get_at(0), None);
    assert_eq!(set.get(&key("gamma")).map(String::as_str), Some("gamma"));
}

#[test]
fn compact_removes_holes_and_reindexes_remaining_keys() {
    let mut set = DeltaSet::new(0);
    set.insert(key("alpha"));
    set.insert(key("beta"));
    set.insert(key("gamma"));
    set.remove(&key("beta"));

    set.compact();

    assert_eq!(set.get_at(0).map(String::as_str), Some("alpha"));
    assert_eq!(set.get_at(1).map(String::as_str), Some("gamma"));
    assert_eq!(set.get(&key("gamma")).map(String::as_str), Some("gamma"));
    assert_eq!(set.insert(key("delta")), Some(2));
    assert_eq!(set.get(&key("delta")).map(String::as_str), Some("delta"));
}
