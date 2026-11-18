use std::collections::HashMap;

use alloy_evm::block::BlockExecutionError;
use alloy_evm::revm::context_interface::result::ResultAndState;
use alloy_evm::revm::state::{Account, EvmState};
use op_revm::OpHaltReason as EvmHaltReason;

use super::ID;
use crate::crdt::Scalar;
use crate::execution::{
    Property, RecorderOutput, ScalarAdapter, ScalarKeyCodec, ScalarKind, key::Key,
    output::ExecutionOutput,
};

use crate::{
    crdt::state::{
        DeltaOp, Tracked,
        op::Operations,
        status::{Status, Tag},
        value::ValueSnapshots,
    },
    namespace::{NamespaceCodec, crdt::CrdtKeyCodec},
    planner::{
        meta::MetaKeyCodec,
        resolver::{CommitDecision, CommitVerdict},
    },
};
use alloy_primitives::{Address, I256, U256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HaltReason {
    Evm(EvmHaltReason),
    Custom(String),
}

/// A job's execution result, commit verdict, receipt, and inspector_record state reads.
pub struct JobResult<R, H = HaltReason> {
    pub id: ID,
    raw_result_and_state: Result<ResultAndState<H>, BlockExecutionError>,
    commit_result: Option<Result<ResultAndState<H>, BlockExecutionError>>,
    pub inspector_record: RecorderOutput, // From the inspector.
    pub receipt: Option<R>,
    pub sender: Address,
    pub fee: U256,
    pub fee_recipients: Vec<(Address, U256)>,
    pub verdict: CommitVerdict<Key>, // Commit decision and conflict diagnostics
}

impl<R, H> JobResult<R, H> {
    /// Returns the execution result and verdict-filtered transitions, or an error for a pending verdict.
    pub fn result_and_state(&mut self) -> &Result<ResultAndState<H>, BlockExecutionError>
    where
        H: Clone,
    {
        if self.raw_result_and_state.is_err() {
            return &self.raw_result_and_state;
        }

        // If the commit decision is still pending, return an error indicating so.
        if self.verdict.decision == CommitDecision::Pending {
            self.commit_result = Some(Err(BlockExecutionError::msg(format!(
                "job {:?} has a pending verdict",
                self.id
            ))));
            return self.commit_result.as_ref().unwrap();
        }

        // If the commit decision is accepted, return the raw result and state as-is.
        if self.verdict.decision == CommitDecision::Accepted {
            return &self.raw_result_and_state;
        }

        let executed = self.raw_result_and_state.as_ref().unwrap();
        let result = executed.result.clone();
        self.commit_result = Some(
            self.retained_state()
                .map(|state| ResultAndState::new(result, state))
                .map_err(BlockExecutionError::other),
        );
        self.commit_result.as_ref().unwrap()
    }

    pub(crate) fn gas_used(&self) -> Option<u64> {
        self.raw_result_and_state
            .as_ref()
            .ok()
            .map(|executed| executed.result.tx_gas_used())
    }

    pub fn new(
        id: ID,
        result_and_state: Result<ResultAndState<H>, BlockExecutionError>,
        receipt: Option<R>,
        inspector_record: RecorderOutput,
    ) -> Self {
        Self {
            id,
            raw_result_and_state: result_and_state,
            commit_result: None,
            receipt,
            inspector_record,
            sender: Address::ZERO,
            fee: U256::ZERO,
            fee_recipients: Vec::new(),
            verdict: CommitVerdict {
                id,
                decision: CommitDecision::Pending,
                conflicts: Vec::new(),
            },
        }
    }
    /// Combines inspector_record operations form the inspector with EVM snapshots for the checker.
    pub fn to_records(&self, id: ID) -> Vec<ExecutionOutput<Key, Tracked<Status, Status>>> {
        let Ok(executed) = &self.raw_result_and_state else {
            return Vec::new();
        };
        let mut unique_accessed: HashMap<Key, Tracked<Status, Status>> = HashMap::new();

        // Add inspector_record operations to the unique accessed map.
        for item in &self.inspector_record {
            let mut tracked = item.data.clone_with_states(ValueSnapshots {
                original: item.data.original().clone(),
                current: item.data.current().clone(),
            });
            tracked.id = id.global;
            unique_accessed.insert(item.key.clone(), tracked);
        }

        // Get unique accessed records from the evmState and
        // combine them with the inspector_record operations.
        for (key, mut journal_records) in self.delta_records(id.global) {
            // At this point, the unique accessed map only
            // have keys from the inspector_record operations.
            if let Some(inspector_record) = unique_accessed.get(&key) {
                // Add inspector_record counts to the journal-derived counts.
                let (journal, inspector) =
                    (&mut journal_records.operations, inspector_record.operations);
                journal.reads = (journal.reads.count() + inspector.reads.count()).into();
                journal.writes = (journal.writes.count() + inspector.writes.count()).into();
                journal.deltas = (journal.deltas.count() + inspector.deltas.count()).into();
                journal.removes = (journal.removes.count() + inspector.removes.count()).into();
                journal.existence_checks =
                    (journal.existence_checks.count() + inspector.existence_checks.count()).into();
            }
            unique_accessed.insert(key, journal_records);
        }
        unique_accessed
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
        // Collect account delta records for the job.
        let mut records = self.account_delta_records(id);

        // Collect storage delta records for the job.
        records.extend(self.slot_delta_records(id));
        records
    }

    /// Collects account records from the result state.
    fn account_delta_records(&self, id: u64) -> Vec<(Key, Tracked<Status, Status>)> {
        let Ok(result_and_state) = &self.raw_result_and_state else {
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
                    Tag::Default // Two transactions write the same default value with no conflict.
                } else {
                    Tag::Stripped
                }
            } else {
                Tag::Missing
            };

            // Balance changes are exported as a bounded U256 scalar for the sum checker.
            let after = account.info.balance;
            if account.original_info().balance != after {
                let mut operations = Operations::default();
                operations.deltas.increment();
                let delta = if after > account.original_info().balance {
                    DeltaOp::Add(after - account.original_info().balance)
                } else {
                    DeltaOp::Sub(account.original_info().balance - after)
                };
                records.push((
                    Key::Balance(*address),
                    Tracked {
                        id,
                        value: ValueSnapshots {
                            original: Status::Tag(original),
                            current: Status::Value(
                                Scalar {
                                    min: U256::ZERO,
                                    max: U256::MAX,
                                    value: account.original_info().balance,
                                    delta: Some(delta),
                                }
                                .into(),
                            ),
                        },
                        operations,
                    },
                ));
            }

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
    /// It need to handle both original and current storage slot values.
    fn slot_delta_records(&self, id: u64) -> Vec<(Key, Tracked<Status, Status>)> {
        let Ok(result_and_state) = &self.raw_result_and_state else {
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
                    let slots = ScalarAdapter::new(*slot_key);
                    let keys = [
                        slots.min,
                        slots.max,
                        slots.value,
                        slots.pos_delta,
                        slots.neg_delta,
                    ];
                    if keys.iter().any(|key| {
                        account
                            .storage
                            .get(key)
                            .is_some_and(|slot| slot.is_changed())
                    }) {
                        operations.deltas.increment();
                    }
                    let [min, max, base, positive, negative] = keys.map(|key| {
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
                } else if !value.is_changed() {
                    Status::Tag(original)
                } else if !value.original_value().is_zero()
                    && value.present_value().is_zero()
                    && NamespaceCodec::decode(*slot_key).is_none()
                {
                    // For evm native slot. None zero => Zero is considered a removal.
                    operations.removes.increment();
                    Status::Tag(Tag::Deleted)
                } else {
                    // All
                    if CrdtKeyCodec::decode(*slot_key).is_some() || MetaKeyCodec::is_crdt(*slot_key)
                    {
                        operations.deltas.increment();
                    } else {
                        operations.writes.increment();
                    }
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

    // When a transaction is rejected, we only want to keep the fee and
    // nonce changes in the state. This fn constructs a new EvmState
    // containing only the fee and nonce updates.
    fn retained_state(&self) -> Result<EvmState, std::io::Error> {
        let Ok(executed) = &self.raw_result_and_state else {
            return Ok(EvmState::default());
        };

        let mut state = EvmState::default();
        // Keep only fee and nonce transitions from the original execution accounts.
        if let Some(sender) = executed.state.get(&self.sender) {
            let mut account = Account::from(sender.original_info()).with_touched_mark();
            account.info.nonce = sender.info.nonce;
            account.info.balance = account.info.balance.checked_sub(self.fee).ok_or_else(|| {
                std::io::Error::other("insufficient balance for rejected transaction fee")
            })?;
            state.insert(self.sender, account);
        }

        // Transaction may contain multiple transfer recipients for (OP only fees)
        // Credit each recipient on its baseline account, or on the sender above.
        for &(recipient, amount) in &self.fee_recipients {
            if amount.is_zero() {
                continue;
            }
            let original = executed
                .state
                .get(&recipient)
                .expect("a fee account is present in the executed state")
                .original_info();
            let account = state
                .entry(recipient)
                .or_insert_with(|| Account::from(original).with_touched_mark());
            account.info.balance = account
                .info
                .balance
                .checked_add(amount)
                .ok_or_else(|| std::io::Error::other("fee recipient balance overflow"))?;
        }
        Ok(state)
    }
}

#[cfg(test)]
#[path = "../tests/job_result.rs"]
mod tests;
