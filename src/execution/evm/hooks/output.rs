use alloy_primitives::Bytes;

/// Result produced by an Arcology call handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallOutput {
    pub status: ExitStatus,
    pub gas_used: u64,
    pub output: Bytes,
}

impl CallOutput {
    /// Builds a successful return with its gas usage and return data.
    pub fn returned(gas_used: u64, output: impl Into<Bytes>) -> Self {
        Self {
            status: ExitStatus::Return,
            gas_used,
            output: output.into(),
        }
    }

    /// Builds a contract revert with its gas usage and revert data.
    pub fn reverted(gas_used: u64, output: impl Into<Bytes>) -> Self {
        Self {
            status: ExitStatus::Revert,
            gas_used,
            output: output.into(),
        }
    }
}

/// How the intercepted call completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    Return,
    Revert,
}
