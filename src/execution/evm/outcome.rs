use alloy_evm::revm::context_interface::result::{HaltReason, ResultAndState};

use super::inspector::StateReads;

/// The EVM output, receipt, and recorded state reads for one transaction.
pub struct TransactionOutcome<R, H = HaltReason> {
    pub result_and_state: ResultAndState<H>,
    pub receipt: R,
    pub reads: StateReads,
}
