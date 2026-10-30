use super::super::sender::Sender;
use alloy_evm::revm::context_interface::Transaction;
use alloy_primitives::{Address, TxKind};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ID {
    /// Stable transaction ID supplied before scheduling.
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
    pub const fn new(global: u64) -> Self {
        Self {
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
    /// Full destination address used for contract-level conflict history.
    pub contract: Option<Address>,
    /// Set by the scheduler in the generated execution plan.
    pub is_deferred: bool,
    /// Bypass this transaction's callee restrictions, while respecting another
    /// transaction's isolated execution. Deferral remains profile-driven.
    pub fully_parallelizable: bool,
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
