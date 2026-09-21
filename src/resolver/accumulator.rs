use std::collections::{HashMap, HashSet};

use super::comparison::ComparableNumeric;
use crate::crdt::state::{Numeric, TransactionOutput};

pub struct Accumulator<K> {
    entries: Vec<Option<Numeric<'static>>>,
    by_key: HashMap<K, Vec<(u64, usize)>>,
}

impl<K> Accumulator<K> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            by_key: HashMap::new(),
        }
    }

    pub fn import(&mut self, numeric_trans: TransactionOutput<K, Numeric<'static>>)
    where
        K: std::cmp::Eq + std::hash::Hash,
    {
        let TransactionOutput { tx_id, records, .. } = numeric_trans;

        for (key, value) in records {
            let entry_ind = self.entries.len();

            self.entries.push(Some(value));
            self.by_key.entry(key).or_default().push((tx_id, entry_ind));
        }
    }

    pub fn accumulate(&mut self) -> Vec<u64> {
        self.sort_entries();

        let mut rejected = HashSet::new();

        self.by_key.values().for_each(|key_entries| {
            let Some(first) = key_entries
                .iter()
                .position(|(_, entry_ind)| self.entries[*entry_ind].is_some())
            else {
                return;
            };

            let first_ind = key_entries[first].1;
            let mut accumulated = self.entries[first_ind].take().unwrap();

            key_entries
                .iter()
                .skip(first + 1)
                .for_each(|(tx_id, entry_ind)| {
                    let Some(value) = self.entries[*entry_ind].take() else {
                        return;
                    };

                    if accumulated.add_delta(&value.delta()).is_err() {
                        rejected.insert(*tx_id);
                    }
                });
        });

        let mut rejected = rejected.into_iter().collect::<Vec<_>>();
        rejected.sort_unstable();
        rejected
    }

    fn sort_entries(&mut self) {
        let entries = &self.entries;

        self.by_key.values_mut().for_each(|key_entries| {
            key_entries.sort_unstable_by(|(left_tx_id, left_ind), (right_tx_id, right_ind)| {
                let left =
                    ComparableNumeric::new(entries[*left_ind].as_ref().unwrap(), *left_tx_id);
                let right =
                    ComparableNumeric::new(entries[*right_ind].as_ref().unwrap(), *right_tx_id);

                left.compare(&right)
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt::{Crdt, int64::I64, state::DeltaOp, u256::U256};
    use std::borrow::Cow;

    fn u256_value(delta: alloy_primitives::U256) -> Numeric<'static> {
        let mut value = U256::default();
        value.add_delta(&DeltaOp::Add(delta)).unwrap();
        Numeric::U256(Cow::Owned(value))
    }

    fn u256_operation(operation: DeltaOp<alloy_primitives::U256>) -> Numeric<'static> {
        let mut value = U256::default();
        value
            .add_delta(&DeltaOp::Add(alloy_primitives::U256::from(10)))
            .unwrap();
        value.apply_delta();
        value.add_delta(&operation).unwrap();
        Numeric::U256(Cow::Owned(value))
    }

    fn i64_value(delta: i64, lower: i64, upper: i64) -> Numeric<'static> {
        let mut value = I64::new(lower, upper).unwrap();
        value.add_delta(&delta).unwrap();
        Numeric::I64(Cow::Owned(value))
    }

    fn execution<K>(
        tx_id: u64,
        records: Vec<(K, Numeric<'static>)>,
    ) -> TransactionOutput<K, Numeric<'static>> {
        TransactionOutput {
            tx_id,
            gas_used: 0,
            records,
        }
    }

    #[test]
    fn sort_entries_orders_numeric_deltas_then_transaction_id() {
        let mut accumulator = Accumulator::new();
        let saturated = alloy_primitives::U256::from(u64::MAX);

        accumulator.import(execution(
            30,
            vec![(1, u256_value(alloy_primitives::U256::from(5)))],
        ));
        accumulator.import(execution(
            20,
            vec![(1, u256_value(alloy_primitives::U256::from(5)))],
        ));
        accumulator.import(execution(
            1,
            vec![(1, u256_value(saturated + alloy_primitives::U256::from(2)))],
        ));
        accumulator.import(execution(
            90,
            vec![(1, u256_value(saturated + alloy_primitives::U256::from(1)))],
        ));
        accumulator.sort_entries();

        let tx_ids = accumulator.by_key[&1]
            .iter()
            .map(|(tx_id, _)| *tx_id)
            .collect::<Vec<_>>();

        assert_eq!(tx_ids, vec![20, 30, 90, 1]);
    }

    #[test]
    fn sort_entries_accounts_for_delta_sign() {
        let mut accumulator = Accumulator::new();

        accumulator.import(execution(
            30,
            vec![(
                1,
                u256_operation(DeltaOp::Sub(alloy_primitives::U256::from(2))),
            )],
        ));
        accumulator.import(execution(
            40,
            vec![(
                1,
                u256_operation(DeltaOp::Sub(alloy_primitives::U256::from(5))),
            )],
        ));
        accumulator.import(execution(
            20,
            vec![(
                1,
                u256_operation(DeltaOp::Add(alloy_primitives::U256::from(2))),
            )],
        ));
        accumulator.import(execution(
            10,
            vec![(
                1,
                u256_operation(DeltaOp::Add(alloy_primitives::U256::from(2))),
            )],
        ));
        accumulator.import(execution(
            50,
            vec![(
                1,
                u256_operation(DeltaOp::Add(alloy_primitives::U256::from(5))),
            )],
        ));

        accumulator.sort_entries();

        let tx_ids = accumulator.by_key[&1]
            .iter()
            .map(|(tx_id, _)| *tx_id)
            .collect::<Vec<_>>();

        assert_eq!(tx_ids, vec![40, 30, 10, 20, 50]);
    }

    #[test]
    fn accumulate_returns_rejected_transactions_and_continues() {
        let mut accumulator = Accumulator::new();

        accumulator.import(execution(1, vec![(1, i64_value(3, -5, 5))]));
        accumulator.import(execution(2, vec![(1, i64_value(4, -5, 5))]));
        accumulator.import(execution(3, vec![(1, i64_value(5, -5, 5))]));
        accumulator.import(execution(4, vec![(2, i64_value(2, -5, 5))]));
        accumulator.import(execution(5, vec![(2, i64_value(4, -5, 5))]));
        accumulator.import(execution(6, vec![(3, i64_value(5, -5, 5))]));

        let rejected = accumulator.accumulate();

        assert_eq!(rejected, vec![2, 3, 5]);
    }
}
