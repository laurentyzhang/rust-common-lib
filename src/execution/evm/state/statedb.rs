use alloy_evm::revm::{database_interface::EmptyDB, state::EvmState};

use super::overlay::StateOverlay;
use crate::store::StoreError;

#[derive(Debug)]
pub struct StateDb<DB = EmptyDB> {
    pub(super) overlay: StateOverlay,
    pub(super) fallback: Option<DB>,
    pub(super) commit_error: Option<StoreError>,
}

impl StateDb {
    pub fn new(overlay: EvmState) -> Self {
        Self {
            overlay: StateOverlay::new(overlay, StateOverlay::copy_balance_nonce),
            fallback: None,
            commit_error: None,
        }
    }
}

impl Default for StateDb {
    fn default() -> Self {
        Self::new(EvmState::default())
    }
}

impl<DB> StateDb<DB> {
    pub fn with_fallback(overlay: StateOverlay, fallback: Option<DB>) -> Self {
        Self {
            overlay,
            fallback,
            commit_error: None,
        }
    }

    /// Returns the first commit error, which prevents subsequent commits.
    pub fn commit_error(&self) -> Option<&StoreError> {
        self.commit_error.as_ref()
    }

    pub fn inner(&self) -> &EvmState {
        self.overlay.inner()
    }

    pub fn inner_mut(&mut self) -> &mut EvmState {
        self.overlay.inner_mut()
    }

    /// Recover the owned EVM state.
    pub fn into_inner(self) -> EvmState {
        self.overlay.into_inner()
    }
}
