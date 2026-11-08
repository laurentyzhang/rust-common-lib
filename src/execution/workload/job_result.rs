use std::collections::HashMap;

use alloy_evm::block::BlockExecutionError;
use alloy_evm::revm::context_interface::result::ResultAndState;
use op_revm::OpHaltReason as EvmHaltReason;

use super::ID;
use crate::crdt::Scalar;
use crate::execution::{
    Property, ScalarAdapter, ScalarKeyCodec, ScalarKind, StateReads, key::Key,
    output::ExecutionOutput,
};
use crate::{
    crdt::state::{
        DeltaOp, Tracked,
        op::Operations,
        status::{Status, Tag},
        value::ValueSnapshots,
    },
    planner::resolver::CommitVerdict,
};
use alloy_primitives::I256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HaltReason {
    Evm(EvmHaltReason),
    Custom(String),
}

/// A job's execution result, commit verdict, receipt, and recorded state reads.
pub struct JobResult<R, H = HaltReason> {
    pub id: ID,
    pub result_and_state: Result<ResultAndState<H>, BlockExecutionError>,
    pub receipt: Option<R>,
    pub reads: StateReads, // Combined from opcode inspection and API handlers.
    /// Commit decision and conflict diagnostics, assigned after generation conflict selection.
    pub verdict: Option<CommitVerdict<Key>>,
}

impl<R, H> JobResult<R, H> {
    pub fn new(
        id: ID,
        result_and_state: Result<ResultAndState<H>, BlockExecutionError>,
        receipt: Option<R>,
        reads: StateReads,
    ) -> Self {
        Self {
            id,
            result_and_state,
            receipt,
            reads,
            verdict: None,
        }
    }
    /// Merges reads and state changes into checker records with the job's ID and gas.
    pub fn to_records(&self, id: ID) -> Vec<ExecutionOutput<Key, Tracked<Status, Status>>> {
        let Ok(executed) = &self.result_and_state else {
            return Vec::new();
        };
        let mut unique_records: HashMap<Key, Tracked<Status, Status>> = HashMap::new();
        for read in &self.reads {
            let mut tracked = read.data.clone_with_states(ValueSnapshots {
                original: read.data.original().clone(),
                current: read.data.current().clone(),
            });
            tracked.id = id.global;
            unique_records.insert(read.key.clone(), tracked);
        }

        for (key, mut write) in self.delta_records(id.global) {
            if let Some(read) = unique_records.get(&key) {
                write.operations.reads = read.operations.reads;
            }
            unique_records.insert(key, write);
        }
        unique_records
            .into_iter()
            .map(|(key, data)| ExecutionOutput {
                id,
                key,
                gas_used: executed.result.tx_gas_used(),
                accumulated_gas: 0,
                data,
            })
            .collect()
    }

    /// Exports REVM account and storage accesses without consuming the result.
    fn delta_records(&self, id: u64) -> Vec<(Key, Tracked<Status, Status>)> {
        let mut records = self.account_delta_records(id);
        records.extend(self.slot_delta_records(id));
        records
    }

    /// Collects account records from the result state.
    fn account_delta_records(&self, id: u64) -> Vec<(Key, Tracked<Status, Status>)> {
        let Ok(result_and_state) = &self.result_and_state else {
            return Vec::new();
        };
        let mut records = Vec::with_capacity(result_and_state.state.len());

        for (address, account) in &result_and_state.state {
            let mut operations = Operations::default();

            let original = if account.is_loaded_as_not_existing() || account.is_created() {
                Tag::Missing
            } else {
                Tag::Stripped
            };

            let current = if account.is_selfdestructed() {
                operations.removes.increment();
                Tag::Deleted
            } else if account.is_changed() || account.is_created() {
                operations.deltas.increment();
                if account.is_empty() {
                    Tag::Default
                } else {
                    Tag::Stripped
                }
            } else {
                Tag::Missing
            };

            records.push((
                Key::Account(*address),
                Tracked {
                    id,
                    value: ValueSnapshots {
                        original: Status::Tag(original),
                        current: Status::Tag(current),
                    },
                    operations,
                },
            ));
        }
        records
    }

    /// Collects storage records from the result state for access and bound checks.
    fn slot_delta_records(&self, id: u64) -> Vec<(Key, Tracked<Status, Status>)> {
        let Ok(result_and_state) = &self.result_and_state else {
            return Vec::new();
        };
        let mut records = Vec::new();

        for (address, account) in &result_and_state.state {
            for (slot_key, value) in &account.storage {
                let mut operations = Operations::default();

                let original = if value.original_value().is_zero() {
                    Tag::Missing
                } else {
                    Tag::Stripped
                };

                let current = if let Some((kind, property, _)) = ScalarKeyCodec::decode(*slot_key)
                    && property == Property::Value as u8
                {
                    operations.deltas.increment();
                    let slots = ScalarAdapter::new(*slot_key);
                    let [min, max, base, positive, negative] = [
                        slots.min,
                        slots.max,
                        slots.value,
                        slots.pos_delta,
                        slots.neg_delta,
                    ]
                    .map(|key| {
                        account
                            .storage
                            .get(&key)
                            .map(|slot| slot.present_value())
                            .unwrap_or_default()
                    });
                    let delta = match positive.cmp(&negative) {
                        std::cmp::Ordering::Greater => Some(DeltaOp::Add(positive - negative)),
                        std::cmp::Ordering::Less => Some(DeltaOp::Sub(negative - positive)),
                        std::cmp::Ordering::Equal => None,
                    };
                    Status::Value(if kind == ScalarKind::I256 {
                        Scalar {
                            min: I256::from_raw(min),
                            max: I256::from_raw(max),
                            value: I256::from_raw(base),
                            delta,
                        }
                        .into()
                    } else {
                        Scalar {
                            min,
                            max,
                            value: base,
                            delta,
                        }
                        .into()
                    })
                } else if value.present_value().is_zero() {
                    operations.removes.increment();
                    Status::Tag(Tag::Deleted)
                } else {
                    operations.deltas.increment();
                    Status::Tag(Tag::Stripped)
                };

                records.push((
                    Key::Storage(*address, *slot_key),
                    Tracked {
                        id,
                        value: ValueSnapshots {
                            original: Status::Tag(original),
                            current,
                        },
                        operations,
                    },
                ));
            }
        }

        records
    }
}
