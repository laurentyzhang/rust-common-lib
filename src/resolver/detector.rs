use rayon::prelude::*;
use std::collections::HashMap;

use crate::crdt::state::{Marker, Numeric, Tracked, TransactionOutput, Value};

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
            if let Value::Numeric(value) =
                std::mem::replace(&mut tracked.value, Value::Marker(Marker::Stripped))
            {
                numerics_trans.push((key.clone(), tracked.clone_with_values(value.clone(), value)));
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

    pub fn detect_conflicts(&mut self)
    where
        K: Sync,
    {
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
                    left.is_preexisting(),
                    left.is_created(),
                    left.writes,
                    left.deltas,
                    left.existence_checks,
                    left.reads,
                    left.id,
                )
                    .cmp(&(
                        right.is_preexisting(),
                        right.is_created(),
                        right.writes,
                        right.deltas,
                        right.existence_checks,
                        right.reads,
                        right.id,
                    ))
            });
        });
    }
}
