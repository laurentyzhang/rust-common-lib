// use std::collections::Vec;

use crate::crdt::state::{Numeric, Tracked, TransactionOutput, Value};

pub struct ConflictDetector<K> {
    numerics: Vec<(K, Numeric<'static>)>,
    entries: Vec<(K, Tracked<Value<'static>>)>,
}

impl<K> ConflictDetector<K> {
    pub fn new() -> Self {
        Self {
            numerics: Vec::new(),
            entries: Vec::new(),
        }
    }

    pub fn import(&mut self, trans: TransactionOutput<K, Tracked<Value<'static>>>)
    where
        K: Clone,
    {
        self.numerics.extend(trans.records.iter().filter_map(
            |(key, tracked)| match tracked.value() {
                Value::Numeric(value) => Some((key.clone(), value.clone())),
                _ => None,
            },
        ));

        self.entries.extend(trans.records);
    }
}
