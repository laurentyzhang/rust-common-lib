use crate::crdt::{Scalar, Type};
use crate::{
    crdt::{
        Crdt,
        state::{Status, Tracked, value::CrdtValue},
    },
    eu::output::ExecutionOutput,
    planner::resolver::Violation,
    planner::resolver::checkers::util::run_parallel,
    planner::scheduler::ID,
};
use std::collections::{BTreeSet, HashMap};

pub struct SumChecker<K, T: Type> {
    pub(in crate::planner::resolver) entries:
        BTreeSet<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>,
    counts: HashMap<K, usize>,
}

impl<K, T: Type> SumChecker<K, T> {
    pub fn new() -> Self {
        Self {
            entries: BTreeSet::new(),
            counts: HashMap::new(),
        }
    }

    pub fn import(&mut self, records: Vec<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>)
    where
        K: Clone + Ord + std::hash::Hash,
    {
        for record in records {
            *self.counts.entry(record.key.clone()).or_default() += 1;
            self.entries.insert(record);
        }
    }

    pub fn check_bounds(&mut self) -> (Vec<Violation<K>>, BTreeSet<(ID, ID)>)
    where
        K: Ord + std::hash::Hash + Send + Sync,
        T: Send,
        T::Magnitude: Send,
        CrdtValue<'static>: From<Scalar<T>>,
    {
        run_parallel(&mut self.entries, &mut self.counts, Self::do_check)
    }

    fn do_check(
        records: Vec<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>,
    ) -> (Vec<Violation<K>>, BTreeSet<(ID, ID)>)
    where
        K: Ord,
        CrdtValue<'static>: From<Scalar<T>>,
    {
        let mut rejected = Vec::new();
        let mut conflicts = BTreeSet::new();
        let mut records = records.into_iter();
        let Some(first) = records.next() else {
            return (rejected, conflicts);
        };
        let mut contributors = Vec::new();
        if first.data.current().delta().is_some() {
            contributors.push(first.id);
        }
        let mut summed = first.data.into_current();

        for record in records {
            let Some(delta) = record.data.current().delta() else {
                continue;
            };

            if let Err(error) = summed.add_delta(delta) {
                for &id in &contributors {
                    conflicts.insert((id.min(record.id), id.max(record.id)));
                }
                rejected.push((
                    record
                        .map_data(|data| data.map_current(|current| Status::Value(current.into()))),
                    error.into(),
                ));
            } else {
                contributors.push(record.id);
            }
        }
        (rejected, conflicts)
    }
}
