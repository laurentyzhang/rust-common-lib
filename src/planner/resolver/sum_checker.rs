use crate::crdt::{Scalar, Type};
use crate::{
    crdt::{
        Crdt,
        state::{Status, Tracked, value::CrdtValue},
    },
    eu::output::ExecutionOutput,
    planner::resolver::traits::{Checker, Rejected},
};
use std::collections::{BTreeSet, HashMap};

pub struct SumChecker<K, T: Type> {
    pub(super) entries: HashMap<K, BTreeSet<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>>,
}

impl<K, T: Type> SumChecker<K, T> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn import(&mut self, records: Vec<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>)
    where
        K: Clone + Eq + std::hash::Hash,
    {
        for record in records {
            self.entries
                .entry(record.key.clone())
                .or_default()
                .insert(record);
        }
    }

    pub fn check_bounds(&mut self) -> (Vec<Rejected<K>>, BTreeSet<(u64, u64)>)
    where
        K: Send + Sync,
        T: Send,
        T::Magnitude: Send,
        CrdtValue<'static>: From<Scalar<T>>,
    {
        Self::check(&mut self.entries)
    }
}

impl<K, T: Type> Checker<K> for SumChecker<K, T>
where
    CrdtValue<'static>: From<Scalar<T>>,
{
    type Input = ExecutionOutput<K, Tracked<Status, Scalar<T>>>;

    fn check_by_key(records: BTreeSet<Self::Input>) -> (Vec<Rejected<K>>, BTreeSet<(u64, u64)>) {
        let mut rejected = Vec::new();
        let mut conflicts = BTreeSet::new();
        let mut records = records.into_iter();
        let Some(first) = records.next() else {
            return (rejected, conflicts);
        };
        let mut contributors = Vec::new();
        if first.data.current().delta().is_some() {
            contributors.push(first.id.global);
        }
        let mut accumulated = first.data.into_current();

        for record in records {
            let Some(delta) = record.data.current().delta() else {
                continue;
            };
            if let Err(error) = accumulated.add_delta(delta) {
                for &id in &contributors {
                    conflicts.insert((id.min(record.id.global), id.max(record.id.global)));
                }
                rejected.push((
                    record
                        .map_data(|data| data.map_current(|current| Status::Value(current.into()))),
                    error.into(),
                ));
            } else {
                contributors.push(record.id.global);
            }
        }
        (rejected, conflicts)
    }
}
