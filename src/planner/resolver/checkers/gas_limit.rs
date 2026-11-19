use crate::execution::output::ExecutionOutput;
use crate::planner::resolver::{Diagnostic, Reason};
use crate::planner::scheduler::ScheduleKey;
use std::cmp::Reverse;
use std::collections::BTreeMap;

pub struct GasLimitChecker {
    pub gas_limit: u64,
    entries: BTreeMap<(Reverse<u64>, ScheduleKey), u64>,
}

impl GasLimitChecker {
    pub fn new(gas_limit: u64) -> Self {
        Self {
            gas_limit,
            entries: BTreeMap::new(),
        }
    }

    pub fn import<K, T>(&mut self, records: &[ExecutionOutput<K, T>]) {
        for record in records {
            self.entries
                .entry((Reverse(record.id.job), ScheduleKey(record.id)))
                .or_insert(record.gas_used);
        }
    }

    /// Trims the highest job positions first, visiting sequences in ascending order.
    pub fn check<K>(&mut self) -> Vec<Diagnostic<K>> {
        let entries = std::mem::take(&mut self.entries);
        let mut gas: u128 = entries.values().map(|&gas| u128::from(gas)).sum();
        let mut diagnostics = Vec::new();
        for ((_, key), used) in entries {
            if gas <= u128::from(self.gas_limit) {
                break;
            }
            gas -= u128::from(used);
            diagnostics.push(Diagnostic {
                id: key.0,
                peers: Vec::new(),
                reason: Reason::GasLimitExceeded,
            });
        }
        diagnostics
    }
}
