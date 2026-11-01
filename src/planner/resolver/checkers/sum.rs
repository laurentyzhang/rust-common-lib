use crate::crdt::{Scalar, Type};
use crate::{
    crdt::{
        Crdt,
        state::{Status, Tracked, value::CrdtValue},
    },
    eu::output::ExecutionOutput,
    planner::resolver::Rejected,
    planner::resolver::checkers::util::check_all,
    planner::scheduler::ID,
};
use std::collections::{BTreeSet, HashMap};

pub struct SumChecker<K, T: Type> {
    pub(in crate::planner::resolver) entries:
        HashMap<K, BTreeSet<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>>,
}

impl<K, T: Type> SumChecker<K, T> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn import(&mut self, records: Vec<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>)
    where
        K: Clone + Ord + std::hash::Hash,
    {
        for record in records {
            self.entries
                .entry(record.key.clone())
                .or_default()
                .insert(record);
        }
    }

    pub fn check_bounds(&mut self) -> (Vec<Rejected<K>>, BTreeSet<(ID, ID)>)
    where
        K: Ord + Send + Sync,
        T: Send,
        T::Magnitude: Send,
        CrdtValue<'static>: From<Scalar<T>>,
    {
        check_all(&mut self.entries, Self::check_by_key)
    }

    fn check_by_key(
        records: BTreeSet<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>,
    ) -> (Vec<Rejected<K>>, BTreeSet<(ID, ID)>)
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
        let mut accumulated = first.data.into_current();

        for record in records {
            let Some(delta) = record.data.current().delta() else {
                continue;
            };
            if let Err(error) = accumulated.add_delta(delta) {
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
