use rayon::prelude::*;
use std::collections::HashMap;

use crate::crdt::state::{Numeric, Tracked, TransactionOutput, Value};

pub struct ConflictDetector<K> {
    numerics_trans: Vec<TransactionOutput<K, Tracked<Numeric<'static>>>>,
    transitions: HashMap<K, Vec<TransactionRecord<Tracked<Value<'static>>>>>,
}

struct TransactionRecord<V> {
    tx_id: u64,
    gas_used: u64,
    value: V,
}

impl<K> ConflictDetector<K> {
    pub fn new() -> Self {
        Self {
            numerics_trans: Vec::new(),
            transitions: HashMap::new(),
        }
    }

    pub fn import(&mut self, trans: TransactionOutput<K, Tracked<Value<'static>>>)
    where
        K: Clone + Eq + std::hash::Hash,
    {
        let TransactionOutput {
            tx_id,
            gas_used,
            records,
        } = trans;
        let mut numerics_trans = Vec::new();

        for (key, mut tracked) in records {
            if matches!(tracked.value, Value::Numeric(_)) {
                if let Value::Numeric(value) = std::mem::replace(&mut tracked.value, Value::None) {
                    numerics_trans.push((key.clone(), tracked.clone_with_value(value)));
                }
            }

            self.transitions
                .entry(key)
                .or_default()
                .push(TransactionRecord {
                    tx_id,
                    gas_used,
                    value: tracked,
                });
        }

        if !numerics_trans.is_empty() {
            self.numerics_trans.push(TransactionOutput {
                tx_id,
                gas_used,
                records: numerics_trans,
            });
        }
    }

    pub fn detect_conflicts(&mut self) {
        self.sort_transactions();
    }

    pub fn sort_transactions(&mut self)
    where
        K: Sync,
    {
        self.transitions.par_iter_mut().for_each(|(_, records)| {
            records.sort_unstable_by(|left, right| {
                let left = &left.value;
                let right = &right.value;

                (
                    left.preexisting,
                    left.is_new,
                    left.writes,
                    left.creates,
                    left.tombstone,
                    left.deltas,
                    left.checks,
                    left.reads,
                    left.id,
                )
                    .cmp(&(
                        right.preexisting,
                        right.is_new,
                        right.writes,
                        right.creates,
                        right.tombstone,
                        right.deltas,
                        right.checks,
                        right.reads,
                        right.id,
                    ))
            });
        });
    }
}
