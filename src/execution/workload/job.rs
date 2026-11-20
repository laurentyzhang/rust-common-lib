use super::JobResult;
use crate::planner::meta::{CalleeId, Meta};
use crate::planner::scheduler::sender::Sender;
use alloy_evm::{
    Evm,
    block::{BlockExecutionError, BlockExecutor, ExecutableTx, TxResult},
    revm::context_interface::{Cfg, Transaction},
    revm::database_interface::WrapDatabaseRef,
    revm::{Database, DatabaseRef},
};
use alloy_op_evm::block::OpTxEnv;
use alloy_primitives::{Address, TxKind, U256};
use op_alloy::consensus::DEPOSIT_TX_TYPE_ID;
use op_revm::{L1BlockInfo, constants::*};
use std::ops::DerefMut;

use crate::execution::{ArcologyEvm, ArcologyTx, Recorder};

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
    /// L1-originated OP deposit transaction.
    pub is_l1_deposit: bool,
    /// Full destination address used for contract metadata storage.
    pub contract: Option<Address>,
    pub meta: Meta,
}

impl<Tx> Job<Tx> {
    pub(crate) fn id(&self) -> Option<CalleeId> {
        (self.contract.is_some() || self.meta.id != CalleeId::default()).then_some(self.meta.id)
    }

    /// Executes one transaction, commits it to the database, and returns its raw outcome.
    pub fn run<E, DB>(self, executor: &mut E) -> JobResult<E::Receipt, <E::Evm as Evm>::HaltReason>
    where
        E: BlockExecutor,
        E::Evm: Evm<DB = DB, Tx = ArcologyTx> + DerefMut<Target = ArcologyEvm<DB, Recorder>>,
        E::Receipt: Clone,
        DB: Database + DatabaseRef + std::fmt::Debug,
        Tx: ExecutableTx<E>,
    {
        executor.evm_mut().reset_reads(self.id.global);
        executor.evm_mut().set_deferred(self.meta.is_deferred);
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
                return JobResult::new(
                    self.id,
                    Err(error),
                    None,
                    executor.evm_mut().drain_recorder(),
                );
            }
        };
        let receipt = executor
            .receipts()
            .last()
            .cloned()
            .expect("a committed transaction has a receipt");
        let inspector_records = executor.evm_mut().drain_recorder();
        let executed = result_and_state.expect("a successful execution has a result");
        let gas = U256::from(executed.result.tx_gas_used());
        let mut raw = JobResult::new(self.id, Ok(executed), Some(receipt), inspector_records);
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
        raw
    }
}

impl<TxEnv: Transaction, R> Job<(TxEnv, R)> {
    /// Builds a job from an executable transaction.
    pub fn new(id: ID, (tx_env, recovered): (TxEnv, R)) -> Self {
        let (contract, callee) = match tx_env.kind() {
            TxKind::Call(to) => {
                let mut selector = [0u8; 4];
                let input = tx_env.input();
                let n = input.len().min(4);
                selector[..n].copy_from_slice(&input[..n]);
                (Some(to), CalleeId::new(to, selector))
            }
            TxKind::Create => (None, CalleeId::default()),
        };
        Self {
            id,
            sender: tx_env.caller().0.0,
            nonce: tx_env.nonce(),
            is_l1_deposit: tx_env.tx_type() == DEPOSIT_TX_TYPE_ID,
            contract,
            meta: Meta {
                id: callee,
                ..Meta::default()
            },
            tx: (tx_env, recovered),
        }
    }
}
