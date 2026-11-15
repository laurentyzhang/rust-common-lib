use std::collections::HashMap;

use alloy_evm::block::BlockExecutionError;
use alloy_evm::revm::context_interface::result::ResultAndState;
use alloy_evm::revm::state::{Account, EvmState};
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
    planner::resolver::{CommitDecision, CommitVerdict},
};
use alloy_primitives::{Address, I256, U256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HaltReason {
    Evm(EvmHaltReason),
    Custom(String),
}

/// A job's execution result, commit verdict, receipt, and recorded state reads.
pub struct JobResult<R, H = HaltReason> {
    pub id: ID,
    raw_result_and_state: Result<ResultAndState<H>, BlockExecutionError>,
    commit_result: Option<Result<ResultAndState<H>, BlockExecutionError>>,
    pub receipt: Option<R>,
    pub reads: StateReads, // Combined from opcode inspection and API handlers.
    pub sender: Address,
    pub fee: U256,
    pub fee_recipients: Vec<(Address, U256)>,
    pub verdict: CommitVerdict<Key>, // Commit decision and conflict diagnostics
}

#[cfg(test)]
#[path = "../tests/job_result.rs"]
mod tests;

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
            self.fee_nonce_only_state()
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
        reads: StateReads,
    ) -> Self {
        Self {
            id,
            raw_result_and_state: result_and_state,
            commit_result: None,
            receipt,
            reads,
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
    /// Merges reads and state changes into checker records with the job's ID and gas.
    pub fn to_records(&self, id: ID) -> Vec<ExecutionOutput<Key, Tracked<Status, Status>>> {
        let Ok(executed) = &self.raw_result_and_state else {
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

    fn fee_nonce_only_state(&self) -> Result<EvmState, std::io::Error> {
        let Ok(executed) = &self.raw_result_and_state else {
            return Ok(EvmState::default());
        };

        let mut state = EvmState::default();
        // Keep only fee and nonce transitions from the original execution accounts.
        if let Some(sender) = executed.state.get(&self.sender) {
            let mut account = Account::from(sender.original_info()).with_touched_mark();
            let nonce_delta = sender
                .info
                .nonce
                .checked_sub(sender.original_info().nonce)
                .ok_or_else(|| std::io::Error::other("sender nonce decreased"))?;
            account.info.nonce = account
                .info
                .nonce
                .checked_add(nonce_delta)
                .ok_or_else(|| std::io::Error::other("sender nonce overflow"))?;
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
