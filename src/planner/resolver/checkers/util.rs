use crate::planner::resolver::Rejected;
use crate::planner::scheduler::ID;
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};

/// Consumes each key's records in parallel and sorts rejections by transaction ID.
/// The map retains its keys with empty record sets.
pub fn check_all<K, T>(
    records_by_key: &mut HashMap<K, BTreeSet<T>>,
    check: impl Fn(BTreeSet<T>) -> (Vec<Rejected<K>>, BTreeSet<(ID, ID)>) + Sync,
) -> (Vec<Rejected<K>>, BTreeSet<(ID, ID)>)
where
    K: Send + Sync,
    T: Send,
{
    let results = records_by_key
        .par_iter_mut()
        .map(|(_, records)| check(std::mem::take(records)))
        .collect::<Vec<_>>();

    let mut rejected = Vec::new();
    let mut conflicts = BTreeSet::new();
    for (records, pairs) in results {
        rejected.extend(records);
        conflicts.extend(pairs);
    }
    rejected.sort_unstable_by_key(|(record, _)| record.id.global);
    (rejected, conflicts)
}
