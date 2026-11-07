use crate::execution::output::ExecutionOutput;
use crate::planner::resolver::Diagnostic;
use crate::planner::scheduler::ID;
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};

/// Groups ordered records using their counts, then checks each key in parallel.
pub fn run_parallel<K, T>(
    entries: &mut BTreeSet<ExecutionOutput<K, T>>,
    counts: &mut HashMap<K, usize>,
    check: impl Fn(Vec<ExecutionOutput<K, T>>) -> (Vec<Diagnostic<K>>, BTreeSet<(ID, ID)>) + Sync,
) -> (Vec<Diagnostic<K>>, BTreeSet<(ID, ID)>)
where
    K: Eq + std::hash::Hash + Send + Sync,
    T: Send,
{
    let mut entries = std::mem::take(entries).into_iter();
    let mut groups = Vec::with_capacity(counts.len());
    while let Some(first) = entries.next() {
        let count = counts.remove(&first.key).unwrap();
        let mut records = Vec::with_capacity(count);
        records.push(first);
        records.extend(entries.by_ref().take(count - 1));
        groups.push(records);
    }
    let results = groups
        .into_par_iter()
        .map(|records| check(records))
        .collect::<Vec<_>>();

    let mut diagnostics = Vec::new();
    let mut conflicts = BTreeSet::new();
    for (records, pairs) in results {
        diagnostics.extend(records);
        conflicts.extend(pairs);
    }
    (diagnostics, conflicts)
}
