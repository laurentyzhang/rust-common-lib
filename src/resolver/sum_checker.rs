use super::traits::{Checker, Rejected};
use crate::crdt::state::{Numeric, Status, Tracked, value::Value};
use crate::execution::output::ExecutionOutput;
use std::collections::{BTreeSet, HashMap};

pub struct SumChecker<K> {
    entries: HashMap<K, BTreeSet<ExecutionOutput<K, Tracked<Status, Numeric<'static>>>>>,
}

impl<K> SumChecker<K> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn import(
        &mut self,
        numeric_trans: HashMap<K, BTreeSet<ExecutionOutput<K, Tracked<Status, Numeric<'static>>>>>,
    ) where
        K: std::cmp::Eq + std::hash::Hash,
    {
        for (key, mut records) in numeric_trans {
            self.entries.entry(key).or_default().append(&mut records);
        }
    }

    pub fn accumulate(&mut self) -> Vec<Rejected<K>>
    where
        K: Send + Sync,
    {
        Self::find_rejections(&mut self.entries)
    }
}

impl<K> Checker<K> for SumChecker<K> {
    type Input = ExecutionOutput<K, Tracked<Status, Numeric<'static>>>;

    fn find_rejections_by_key(records: BTreeSet<Self::Input>) -> Vec<Rejected<K>> {
        let mut rejected = Vec::new();
        let mut records = records.into_iter();
        let Some(first) = records.next() else {
            return rejected;
        };

        let mut accumulated = first.data.into_current();

        for record in records {
            if let Err(error) = accumulated.add_delta(&record.data.current().delta()) {
                rejected.push((
                    record.map_data(|data| {
                        data.map_current(|current| Status::Value(Value::Numeric(current)))
                    }),
                    error.into(),
                ));
            }
        }
        rejected
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
