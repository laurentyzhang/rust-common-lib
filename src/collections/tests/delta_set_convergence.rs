use super::DeltaSet;

fn key(value: &str) -> String {
    value.to_owned()
}

fn values(set: &DeltaSet<String>) -> Vec<String> {
    let mut values: Vec<_> = set.elements.iter().flatten().cloned().collect();
    values.sort();
    values
}

fn commit(set: &mut DeltaSet<String>) {
    set.committed = set.keys.clone();
}

#[test]
fn three_sets_converge_after_add_and_remove_deltas() {
    let mut a = DeltaSet::new(0);
    let mut b = DeltaSet::new(0);
    let mut c = DeltaSet::new(0);

    assert!(a.committed.is_empty());
    assert!(b.committed.is_empty());
    assert!(c.committed.is_empty());

    for value in ["one", "two", "three"] {
        a.insert(key(value));
    }
    for value in ["three", "four", "five"] {
        b.insert(key(value));
    }
    for value in ["five", "six", "one"] {
        c.insert(key(value));
    }

    let (a_added, a_removed) = a.diff();
    let (b_added, b_removed) = b.diff();
    let (c_added, c_removed) = c.diff();

    for set in [&mut a, &mut b, &mut c] {
        set.apply_delta(&a_removed, &a_added);
        set.apply_delta(&b_removed, &b_added);
        set.apply_delta(&c_removed, &c_added);
    }

    assert_eq!(values(&a), values(&b));
    assert_eq!(values(&b), values(&c));

    commit(&mut a);
    commit(&mut b);
    commit(&mut c);

    for value in ["one", "two"] {
        a.remove(&key(value));
    }
    for value in ["two", "three"] {
        b.remove(&key(value));
    }
    for value in ["three", "four"] {
        c.remove(&key(value));
    }

    let (a_added, a_removed) = a.diff();
    let (b_added, b_removed) = b.diff();
    let (c_added, c_removed) = c.diff();

    for set in [&mut a, &mut b, &mut c] {
        set.apply_delta(&a_removed, &a_added);
        set.apply_delta(&b_removed, &b_added);
        set.apply_delta(&c_removed, &c_added);
    }

    assert_eq!(values(&a), values(&b));
    assert_eq!(values(&b), values(&c));
    assert_eq!(values(&a), vec![key("five"), key("six")]);
}
