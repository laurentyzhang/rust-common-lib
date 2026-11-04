use crate::planner::scheduler::ID;
use std::collections::{BTreeSet, HashMap};

/// Keeps sequence prefixes, rolling back the cheaper remaining suffix per conflict.
pub(super) fn select(
    sequences: &[Vec<(ID, u64)>],
    pairs: &BTreeSet<(ID, ID)>,
    rejected: impl IntoIterator<Item = ID>,
) -> BTreeSet<ID> {
    let positions: HashMap<_, _> = sequences
        .iter()
        .enumerate()
        .flat_map(|(sequence, jobs)| {
            jobs.iter()
                .enumerate()
                .map(move |(position, (id, _))| (*id, (sequence, position)))
        })
        .collect();
    let mut ends: Vec<_> = sequences.iter().map(Vec::len).collect();

    // Mandatory rejections also discard dependent jobs in the same sequence.
    for id in rejected {
        let (sequence, position) = positions[&id];
        ends[sequence] = ends[sequence].min(position);
    }

    for (left, right) in pairs {
        let (left_sequence, left_position) = positions[left];
        let (right_sequence, right_position) = positions[right];
        if left_position >= ends[left_sequence] || right_position >= ends[right_sequence] {
            continue;
        }

        // Count only gas that has not already been discarded by an earlier rollback.
        let cost = |sequence: usize, position: usize| {
            let jobs = &sequences[sequence];
            let before = if position == 0 {
                0
            } else {
                jobs[position - 1].1
            };
            jobs[ends[sequence] - 1].1 - before
        };
        let left_cost = cost(left_sequence, left_position);
        let right_cost = cost(right_sequence, right_position);
        // Equal costs keep the earlier global ID.
        if left_cost < right_cost || (left_cost == right_cost && left > right) {
            ends[left_sequence] = left_position;
        } else {
            ends[right_sequence] = right_position;
        }
    }

    sequences
        .iter()
        .zip(ends)
        .flat_map(|(jobs, end)| jobs[..end].iter().map(|(id, _)| *id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(global: u64, sequence: u64, job: u64) -> ID {
        ID {
            sequence,
            job,
            ..ID::new(global)
        }
    }

    #[test]
    fn rolling_back_one_job_saves_both_peers() {
        let (a, b, c) = (id(1, 0, 0), id(2, 1, 0), id(3, 2, 0));
        let sequences = vec![vec![(a, 5)], vec![(b, 10)], vec![(c, 20)]];
        let pairs = BTreeSet::from([(a, b), (a, c)]);
        assert_eq!(select(&sequences, &pairs, []), BTreeSet::from([b, c]));
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn compares_whole_suffix_costs_including_the_conflicting_job() {
        let (a, b, tail) = (id(1, 0, 0), id(2, 1, 0), id(3, 0, 1));
        let sequences = vec![vec![(a, 1), (tail, 101)], vec![(b, 50)]];
        assert_eq!(
            select(&sequences, &BTreeSet::from([(a, b)]), []),
            BTreeSet::from([a, tail])
        );
    }

    #[test]
    fn recalculates_cost_after_shortening_a_sequence() {
        let (a, tail, b, c) = (id(2, 0, 0), id(1, 0, 1), id(3, 1, 0), id(4, 2, 0));
        let sequences = vec![vec![(a, 20), (tail, 120)], vec![(b, 1_000)], vec![(c, 50)]];
        let pairs = BTreeSet::from([(tail, b), (a, c)]);
        assert_eq!(select(&sequences, &pairs, []), BTreeSet::from([b, c]));
    }

    #[test]
    fn mandatory_rejection_removes_its_suffix_and_resolves_its_pairs() {
        let (a, tail, b) = (id(1, 0, 0), id(2, 0, 1), id(3, 1, 0));
        let sequences = vec![vec![(a, 10), (tail, 110)], vec![(b, 1)]];
        assert_eq!(
            select(&sequences, &BTreeSet::from([(tail, b)]), [tail]),
            BTreeSet::from([a, b])
        );
    }

    #[test]
    fn equal_costs_keep_the_earlier_id() {
        let (a, b) = (id(1, 0, 0), id(2, 1, 0));
        let sequences = vec![vec![(a, 0)], vec![(b, 0)]];
        for pair in [(a, b), (b, a)] {
            assert_eq!(
                select(&sequences, &BTreeSet::from([pair]), []),
                BTreeSet::from([a])
            );
        }
        assert!(select(&[], &BTreeSet::new(), []).is_empty());
    }
}
