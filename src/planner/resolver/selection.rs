use crate::planner::scheduler::ID;
use std::collections::{BTreeSet, HashMap};

/// Keeps jobs before the first rejection in each sequence.
pub(super) fn resolve_conflicts_and_dependencies(
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
#[path = "tests/selection.rs"]
mod tests;
