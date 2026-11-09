use super::JobResult;
use crate::planner::scheduler::sender::Sender;
use alloy_evm::{
    Evm,
    block::{BlockExecutionError, BlockExecutor, ExecutableTx, TxResult},
    revm::context_interface::{Cfg, Transaction},
    revm::database_interface::WrapDatabaseRef,
    revm::{DatabaseRef, state::EvmState},
};
use alloy_op_evm::block::OpTxEnv;
use alloy_primitives::{Address, TxKind, U256};
use op_alloy::consensus::DEPOSIT_TX_TYPE_ID;
use op_revm::{L1BlockInfo, constants::*};
use std::ops::DerefMut;

use crate::execution::{ArcologyEvm, ArcologyTx, ReadRecorder, StateDb, StateOutcome};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ID {
    /// Block number containing this transaction.
    pub block: u64,
    /// Original zero-based transaction position within the block.
    pub global: u64,
    /// Zero-based generation position in the finalized plan.
    pub generation: u64,
    /// Zero-based sequence position within the generation.
    pub sequence: u64,
    /// Zero-based job position within the sequence.
    pub job: u64,
}

impl ID {
    /// Creates an unscheduled ID. The scheduler fills in its plan coordinates.
    pub const fn new(block: u64, global: u64) -> Self {
        Self {
            block,
            global,
            generation: 0,
            sequence: 0,
            job: 0,
        }
    }
}

/// Transaction data supplied to the scheduler before planning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Job<Tx> {
    /// Original executable transaction carried through scheduling.
    pub tx: Tx,
    /// Stable global ID plus the location assigned by the scheduler.
    pub id: ID,
    pub sender: Sender,
    pub nonce: u64,
    pub callee: Option<u64>,
    /// Full destination address used for contract metadata storage.
    pub contract: Option<Address>,
    /// Set by the scheduler in the generated execution plan.
    pub is_deferred: bool,
    /// Bypass this transaction's callee restrictions, while respecting another
    /// transaction's isolated execution. Deferral remains profile-driven.
    pub fully_parallelizable: bool,
}

impl<Tx> Job<Tx> {
    /// Executes one transaction and returns its raw outcome and committed local state.
    pub fn run<E, DB>(
        self,
        executor: &mut E,
    ) -> StateOutcome<JobResult<E::Receipt, <E::Evm as Evm>::HaltReason>, EvmState>
    where
        E: BlockExecutor,
        E::Evm: Evm<DB = StateDb<DB>, Tx = ArcologyTx>
            + DerefMut<Target = ArcologyEvm<StateDb<DB>, ReadRecorder>>,
        E::Receipt: Clone,
        DB: DatabaseRef + std::fmt::Debug,
        Tx: ExecutableTx<E>,
    {
        executor.evm_mut().reset_reads(self.id.global);
        executor.evm_mut().set_deferred(self.is_deferred);
        let (tx, recovered) = self.tx.into_parts();
        let evm = executor.evm_mut().deref_mut();
        let block = evm.block().clone();
        let spec = evm.cfg_env().spec;
        let charge_fees =
            tx.tx_type() != DEPOSIT_TX_TYPE_ID && !evm.cfg_env().is_fee_charge_disabled();
        let l1_info = if charge_fees {
            L1BlockInfo::try_fetch(&mut WrapDatabaseRef(evm.db()), block.number, spec)
                .map_err(BlockExecutionError::other)
        } else {
            Ok(L1BlockInfo::default())
        };
        let sender = tx.caller();
        let gas_price = tx.effective_gas_price(block.basefee as u128);
        let encoded = tx.encoded_bytes().cloned().unwrap_or_default();
        let mut result_and_state = None;
        let execution =
            executor.execute_transaction_with_result_closure((tx, recovered), |result| {
                result_and_state = Some(result.result().clone());
            });
        let mut l1_info = match execution.and(l1_info) {
            Ok(info) => info,
            Err(error) => {
                return StateOutcome {
                    raw: JobResult::new(
                        self.id,
                        Err(error),
                        None,
                        executor.evm_mut().drain_reads(),
                    ),
                    committed: EvmState::default(),
                };
            }
        };
        let receipt = executor
            .receipts()
            .last()
            .cloned()
            .expect("a committed transaction has a receipt");
        let reads = executor.evm_mut().drain_reads();
        let executed = result_and_state.expect("a successful execution has a result");
        let gas = U256::from(executed.result.tx_gas_used());
        let mut raw = JobResult::new(self.id, Ok(executed), Some(receipt), reads);
        raw.sender = sender;
        if charge_fees {
            let l1_fee = l1_info.calculate_tx_l1_cost(&encoded, spec);
            let operator_fee = l1_info.operator_fee_charge(&encoded, gas, spec);
            raw.fee = gas * U256::from(gas_price) + l1_fee + operator_fee;
            raw.fee_recipients = vec![
                (
                    block.beneficiary,
                    gas * U256::from(gas_price.saturating_sub(block.basefee as u128)),
                ),
                (BASE_FEE_RECIPIENT, gas * U256::from(block.basefee)),
                (L1_FEE_RECIPIENT, l1_fee),
                (OPERATOR_FEE_RECIPIENT, operator_fee),
            ];
        }
        let mut committed = executor.evm().db().inner().clone();
        // REVM includes loaded code; expose only actual code updates to the committer.
        for account in committed.values_mut() {
            if account.info.code_hash == account.original_info().code_hash {
                account.info.code = None;
            }
        }
        StateOutcome { raw, committed }
    }
}

impl<TxEnv: Transaction, R> Job<(TxEnv, R)> {
    /// Builds a job from an executable transaction. `callee` is the target's first
    /// 4 address bytes plus the 4-byte selector.
    pub fn new(id: ID, (tx_env, recovered): (TxEnv, R)) -> Self {
        let callee = match tx_env.kind() {
            TxKind::Call(to) => {
                let mut key = [0u8; 8];
                key[..4].copy_from_slice(&to[..4]);
                let input = tx_env.input();
                let n = input.len().min(4);
                key[4..4 + n].copy_from_slice(&input[..n]);
                Some(u64::from_le_bytes(key))
            }
            TxKind::Create => None,
        };
        Self {
            id,
            sender: tx_env.caller().0.0,
            nonce: tx_env.nonce(),
            callee,
            contract: match tx_env.kind() {
                TxKind::Call(address) => Some(address),
                TxKind::Create => None,
            },
            is_deferred: false,
            fully_parallelizable: false,
            tx: (tx_env, recovered),
        }
    }
}
