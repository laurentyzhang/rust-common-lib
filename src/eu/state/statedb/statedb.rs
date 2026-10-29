//! Owned EVM state used by the state trait implementations.

use alloy_evm::revm::state::EvmState;

#[derive(Debug, Default)]
pub struct StateDb {
    state: EvmState,
}

impl StateDb {
    pub fn new(state: EvmState) -> Self {
        Self { state }
    }

    pub fn inner(&self) -> &EvmState {
        &self.state
    }

    pub(super) fn inner_mut(&mut self) -> &mut EvmState {
        &mut self.state
    }

    /// Recover the owned EVM state.
    pub fn into_inner(self) -> EvmState {
        self.state
    }
}
