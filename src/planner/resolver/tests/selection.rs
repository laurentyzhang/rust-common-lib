use super::*;

fn id(global: u64, sequence: u64, job: u64) -> ID {
    ID {
        sequence,
        job,
        ..ID::new(0, global)
    }
}

#[test]
fn rolling_back_one_job_saves_both_peers() {
    let (a, b, c) = (id(1, 0, 0), id(2, 1, 0), id(3, 2, 0));
    let sequences = vec![vec![(a, 5)], vec![(b, 10)], vec![(c, 20)]];
    let pairs = BTreeSet::from([(a, b), (a, c)]);
    assert_eq!(
        resolve_conflicts_and_dependencies(&sequences, &pairs, []),
        BTreeSet::from([b, c])
    );
    assert_eq!(pairs.len(), 2);
}

#[test]
fn compares_whole_suffix_costs_including_the_conflicting_job() {
    let (a, b, tail) = (id(1, 0, 0), id(2, 1, 0), id(3, 0, 1));
    let sequences = vec![vec![(a, 1), (tail, 101)], vec![(b, 50)]];
    assert_eq!(
        resolve_conflicts_and_dependencies(&sequences, &BTreeSet::from([(a, b)]), []),
        BTreeSet::from([a, tail])
    );
}

#[test]
fn recalculates_cost_after_shortening_a_sequence() {
    let (a, tail, b, c) = (id(2, 0, 0), id(1, 0, 1), id(3, 1, 0), id(4, 2, 0));
    let sequences = vec![vec![(a, 20), (tail, 120)], vec![(b, 1_000)], vec![(c, 50)]];
    let pairs = BTreeSet::from([(tail, b), (a, c)]);
    assert_eq!(
        resolve_conflicts_and_dependencies(&sequences, &pairs, []),
        BTreeSet::from([b, c])
    );
}

#[test]
fn mandatory_rejection_removes_its_suffix_and_resolves_its_pairs() {
    let (a, tail, b) = (id(1, 0, 0), id(2, 0, 1), id(3, 1, 0));
    let sequences = vec![vec![(a, 10), (tail, 110)], vec![(b, 1)]];
    assert_eq!(
        resolve_conflicts_and_dependencies(&sequences, &BTreeSet::from([(tail, b)]), [tail]),
        BTreeSet::from([a, b])
    );
}

#[test]
fn equal_costs_keep_the_earlier_id() {
    let (a, b) = (id(1, 0, 0), id(2, 1, 0));
    let sequences = vec![vec![(a, 0)], vec![(b, 0)]];
    for pair in [(a, b), (b, a)] {
        assert_eq!(
            resolve_conflicts_and_dependencies(&sequences, &BTreeSet::from([pair]), []),
            BTreeSet::from([a])
        );
    }
    assert!(resolve_conflicts_and_dependencies(&[], &BTreeSet::new(), []).is_empty());
}
