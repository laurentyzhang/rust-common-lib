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
mod tests {
    use super::*;
    use crate::crdt::{
        Crdt,
        int64::I64,
        state::{Delta, DeltaOp, Value, status::Tag, value::Values},
        u256::U256,
        uint64::U64,
    };
    use crate::resolver::reason::Reason;
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

    fn u64_value(delta: u64) -> Numeric<'static> {
        let mut value = U64::default();
        value.add_delta(&DeltaOp::Add(delta)).unwrap();
        Numeric::U64(Cow::Owned(value))
    }

    fn u64_operation(operation: DeltaOp<u64>) -> Numeric<'static> {
        let mut value = U64::default();
        value.add_delta(&DeltaOp::Add(10)).unwrap();
        value.apply_delta();
        value.add_delta(&operation).unwrap();
        Numeric::U64(Cow::Owned(value))
    }

    fn i64_value(delta: i64, lower: i64, upper: i64) -> Numeric<'static> {
        let mut value = I64::new(lower, upper).unwrap();
        value.add_delta(&delta).unwrap();
        Numeric::I64(Cow::Owned(value))
    }

    fn execution<K>(
        tx_id: u64,
        records: Vec<(K, Numeric<'static>)>,
    ) -> HashMap<K, BTreeSet<ExecutionOutput<K, Tracked<Status, Numeric<'static>>>>>
    where
        K: Clone + Eq + std::hash::Hash,
    {
        let mut outputs = HashMap::new();

        for (key, value) in records {
            outputs
                .entry(key.clone())
                .or_insert_with(BTreeSet::new)
                .insert(ExecutionOutput {
                    id: tx_id,
                    key,
                    gas_used: 0,
                    data: Tracked::<Value<'static>, Value<'static>>::new_owned_empty(tx_id)
                        .clone_with_states(Values {
                            original: Status::Tag(Tag::Missing),
                            current: value,
                        }),
                });
        }

        outputs
    }

    fn assert_mixed_sign_rejections(
        base: Numeric<'static>,
        deltas: Vec<Delta>,
        underflow_id: u64,
        overflow_ids: &[u64],
    ) {
        // Each transaction starts from the same committed value and is valid
        // on its own. Only combining transactions may exceed the type's range.
        let values = deltas
            .iter()
            .map(|delta| {
                let mut value = base.clone();
                value
                    .add_delta(delta)
                    .expect("individual delta must be valid");
                value
            })
            .collect::<Vec<_>>();

        // Check every sorted prefix, so accepted deltas after underflow and
        // the precise point of overflow are verified, in either import order.
        for prefix_len in 1..=values.len() {
            for reverse_import in [false, true] {
                let mut sum_checker = SumChecker::new();
                let mut indices = (0..prefix_len).collect::<Vec<_>>();
                if reverse_import {
                    indices.reverse();
                }
                for index in indices {
                    sum_checker.import(execution(
                        index as u64 + 1,
                        vec![(7, values[index].clone())],
                    ));
                }

                assert_eq!(
                    sum_checker.entries[&7]
                        .iter()
                        .map(|record| record.id)
                        .collect::<Vec<_>>(),
                    (1..=prefix_len as u64).collect::<Vec<_>>(),
                );
                let rejected = sum_checker.accumulate();
                let expected_ids = std::iter::once(underflow_id)
                    .chain(overflow_ids.iter().copied())
                    .filter(|id| *id <= prefix_len as u64)
                    .collect::<Vec<_>>();
                assert_eq!(
                    rejected
                        .iter()
                        .map(|(record, _)| record.id)
                        .collect::<Vec<_>>(),
                    expected_ids,
                    "prefix_len={prefix_len}, reverse_import={reverse_import}",
                );

                for (record, reason) in rejected {
                    assert_eq!(record.key, 7);
                    assert!(matches!(
                        record.data.current(),
                        Status::Value(Value::Numeric(value)) if value == &values[record.id as usize - 1]
                    ));
                    if record.id == underflow_id {
                        assert!(matches!(
                            reason,
                            Reason::NumericError(crate::resolver::reason::NumericError::Underflow(
                                _
                            ))
                        ));
                    } else {
                        assert!(matches!(
                            reason,
                            Reason::NumericError(crate::resolver::reason::NumericError::Overflow(
                                _
                            ))
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn accumulate_i64_mixed_signs_reports_underflow_and_overflow() {
        for (initial, deltas, overflow_ids) in [
            // Pending delta reaches MIN, cancels to zero, then reaches MAX.
            (
                0,
                vec![
                    i64::MIN + 1,
                    i64::MIN + 2,
                    -1,
                    0,
                    1,
                    i64::MAX,
                    i64::MAX,
                    i64::MAX,
                    i64::MAX,
                ],
                [8, 9],
            ),
            // Underflow occurs in stored value + pending delta. Later deltas
            // reach MIN, cancel, reach pending MAX, and overflow pending delta.
            (
                -10,
                vec![
                    i64::MIN + 11,
                    -2,
                    -1,
                    0,
                    1,
                    i64::MAX - 10,
                    i64::MAX,
                    i64::MAX,
                    i64::MAX,
                ],
                [8, 9],
            ),
            // Underflow occurs in pending delta; overflow occurs when adding
            // a representable pending delta (MAX - 2) to the stored value 10.
            (
                10,
                vec![
                    i64::MIN + 1,
                    i64::MIN + 2,
                    -1,
                    0,
                    1,
                    i64::MAX / 2,
                    i64::MAX / 2,
                    i64::MAX / 2,
                    i64::MAX / 2,
                    i64::MAX / 2,
                ],
                [9, 10],
            ),
        ] {
            let mut base = I64::default();
            base.add_delta(&initial).unwrap();
            base.apply_delta();
            assert_mixed_sign_rejections(
                Numeric::I64(Cow::Owned(base)),
                deltas.into_iter().map(Delta::I64).collect(),
                2,
                &overflow_ids,
            );
        }
    }

    #[test]
    fn accumulate_u64_mixed_signs_reports_underflow_and_overflow() {
        let mut base = U64::default();
        base.add_delta(&DeltaOp::Add(10)).unwrap();
        base.apply_delta();

        // 10 - 6 = 4; -5 underflows; -4 reaches zero; +10 cancels;
        // +(MAX - 10) reaches MAX. Both following additions overflow.
        let deltas = [
            DeltaOp::Sub(6),
            DeltaOp::Sub(5),
            DeltaOp::Sub(4),
            DeltaOp::Add(0),
            DeltaOp::Add(10),
            DeltaOp::Add(u64::MAX - 10),
            DeltaOp::Add(u64::MAX - 10),
            DeltaOp::Add(u64::MAX - 10),
        ];
        assert_mixed_sign_rejections(
            Numeric::U64(Cow::Owned(base)),
            deltas.into_iter().map(Delta::U64).collect(),
            2,
            &[7, 8],
        );
    }

    #[test]
    fn accumulate_u256_mixed_signs_reports_underflow_and_overflow() {
        use alloy_primitives::U256 as Number;

        let mut base = U256::default();
        base.add_delta(&DeltaOp::Add(Number::from(10))).unwrap();
        base.apply_delta();

        // Exercise the full 256-bit range with the same recovery and
        // cancellation sequence as U64, including repeated overflow.
        let large = Number::MAX - Number::from(10);
        let deltas = [
            DeltaOp::Sub(Number::from(6)),
            DeltaOp::Sub(Number::from(5)),
            DeltaOp::Sub(Number::from(4)),
            DeltaOp::Add(Number::ZERO),
            DeltaOp::Add(Number::from(10)),
            DeltaOp::Add(large),
            DeltaOp::Add(large),
            DeltaOp::Add(large),
        ];
        assert_mixed_sign_rejections(
            Numeric::U256(Cow::Owned(base)),
            deltas.into_iter().map(Delta::U256).collect(),
            2,
            &[7, 8],
        );
    }

    #[test]
    fn sort_entries_orders_numeric_deltas_then_transaction_id() {
        let mut sum_checker = SumChecker::new();
        let saturated = alloy_primitives::U256::from(u64::MAX);

        sum_checker.import(execution(
            30,
            vec![(1, u256_value(alloy_primitives::U256::from(5)))],
        ));
        sum_checker.import(execution(
            20,
            vec![(1, u256_value(alloy_primitives::U256::from(5)))],
        ));
        sum_checker.import(execution(
            1,
            vec![(1, u256_value(saturated + alloy_primitives::U256::from(2)))],
        ));
        sum_checker.import(execution(
            90,
            vec![(1, u256_value(saturated + alloy_primitives::U256::from(1)))],
        ));
        let tx_ids = sum_checker.entries[&1]
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>();

        assert_eq!(tx_ids, vec![20, 30, 90, 1]);
    }

    #[test]
    fn sort_entries_accounts_for_delta_sign() {
        let mut sum_checker = SumChecker::new();

        sum_checker.import(execution(
            30,
            vec![(
                1,
                u256_operation(DeltaOp::Sub(alloy_primitives::U256::from(2))),
            )],
        ));
        sum_checker.import(execution(
            40,
            vec![(
                1,
                u256_operation(DeltaOp::Sub(alloy_primitives::U256::from(5))),
            )],
        ));
        sum_checker.import(execution(
            20,
            vec![(
                1,
                u256_operation(DeltaOp::Add(alloy_primitives::U256::from(2))),
            )],
        ));
        sum_checker.import(execution(
            10,
            vec![(
                1,
                u256_operation(DeltaOp::Add(alloy_primitives::U256::from(2))),
            )],
        ));
        sum_checker.import(execution(
            50,
            vec![(
                1,
                u256_operation(DeltaOp::Add(alloy_primitives::U256::from(5))),
            )],
        ));

        let tx_ids = sum_checker.entries[&1]
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>();

        assert_eq!(tx_ids, vec![40, 30, 10, 20, 50]);
    }

    #[test]
    fn accumulate_returns_rejected_transactions_and_continues() {
        let mut sum_checker = SumChecker::new();

        sum_checker.import(execution(1, vec![(1, i64_value(3, -5, 5))]));
        sum_checker.import(execution(2, vec![(1, i64_value(4, -5, 5))]));
        sum_checker.import(execution(3, vec![(1, i64_value(5, -5, 5))]));
        sum_checker.import(execution(4, vec![(2, i64_value(2, -5, 5))]));
        sum_checker.import(execution(5, vec![(2, i64_value(4, -5, 5))]));
        sum_checker.import(execution(6, vec![(3, i64_value(5, -5, 5))]));

        let rejected = sum_checker.accumulate();
        let rejected = rejected
            .into_iter()
            .map(|(record, _)| record.id)
            .collect::<Vec<_>>();

        assert_eq!(rejected, vec![2, 3, 5]);
    }

    #[test]
    fn empty_and_single_record_groups_have_no_rejections() {
        let mut sum_checker = SumChecker::<u64>::new();
        assert!(sum_checker.accumulate().is_empty());

        sum_checker.import(execution(1, vec![(7, i64_value(3, -5, 5))]));
        assert!(sum_checker.accumulate().is_empty());
        assert!(sum_checker.entries[&7].is_empty());
        assert!(sum_checker.accumulate().is_empty());
    }

    #[test]
    fn rejection_retains_key_numeric_data_and_reason() {
        let mut sum_checker = SumChecker::new();
        sum_checker.import(execution(1, vec![(7, i64_value(3, -5, 5))]));
        sum_checker.import(execution(2, vec![(7, i64_value(4, -5, 5))]));

        let rejected = sum_checker.accumulate();
        assert_eq!(rejected.len(), 1);

        let (record, reason) = &rejected[0];
        assert_eq!(record.id, 2);
        assert_eq!(record.key, 7);
        assert!(matches!(
            record.data.current(),
            Status::Value(Value::Numeric(Numeric::I64(value)))
                if value.delta().copied() == Some(4)
        ));
        assert!(matches!(
            reason,
            Reason::NumericError(crate::resolver::reason::NumericError::AboveUpperLimit(_))
        ));
    }

    #[test]
    fn same_transaction_rejected_for_multiple_keys_keeps_every_record() {
        let mut sum_checker = SumChecker::new();
        sum_checker.import(execution(1, vec![(10, i64_value(3, -5, 5))]));
        sum_checker.import(execution(2, vec![(20, i64_value(3, -5, 5))]));
        sum_checker.import(execution(
            7,
            vec![(10, i64_value(4, -5, 5)), (20, i64_value(4, -5, 5))],
        ));

        let rejected = sum_checker.accumulate();
        assert_eq!(rejected.len(), 2);
        assert!(rejected.iter().all(|(record, _)| record.id == 7));

        let mut keys = rejected
            .into_iter()
            .map(|(record, _)| record.key)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(keys, vec![10, 20]);
    }

    #[test]
    fn accumulate_reports_arithmetic_overflow() {
        let mut sum_checker = SumChecker::new();
        sum_checker.import(execution(1, vec![(7, u64_value(1))]));
        sum_checker.import(execution(2, vec![(7, u64_value(u64::MAX))]));

        let rejected = sum_checker.accumulate();
        assert_eq!(rejected.len(), 1);

        let (record, reason) = &rejected[0];
        assert_eq!(record.id, 2);
        assert_eq!(record.key, 7);
        assert!(matches!(
            record.data.current(),
            Status::Value(Value::Numeric(Numeric::U64(value)))
                if value.delta() == Some(&DeltaOp::Add(u64::MAX))
        ));
        assert!(matches!(
            reason,
            Reason::NumericError(crate::resolver::reason::NumericError::Overflow(_))
        ));
    }

    #[test]
    fn accumulate_reports_arithmetic_underflow() {
        let mut sum_checker = SumChecker::new();
        sum_checker.import(execution(1, vec![(7, u64_operation(DeltaOp::Sub(6)))]));
        sum_checker.import(execution(2, vec![(7, u64_operation(DeltaOp::Sub(5)))]));

        let rejected = sum_checker.accumulate();
        assert_eq!(rejected.len(), 1);

        let (record, reason) = &rejected[0];
        assert_eq!(record.id, 2);
        assert_eq!(record.key, 7);
        assert!(matches!(
            record.data.current(),
            Status::Value(Value::Numeric(Numeric::U64(value)))
                if value.delta() == Some(&DeltaOp::Sub(5))
        ));
        assert!(matches!(
            reason,
            Reason::NumericError(crate::resolver::reason::NumericError::Underflow(_))
        ));
    }
}
