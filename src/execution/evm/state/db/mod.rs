use alloy_evm::revm::{database_interface::EmptyDB, state::EvmState};

use super::overlay::StateOverlay;
use super::store::OverlayWrite;
use crate::store::StoreError;

mod commit;
mod database;
mod slot;

#[derive(Debug)]
pub struct StateDb<DB = EmptyDB, Store = EvmState> {
    pub(super) overlay: StateOverlay<Store>,
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

impl<DB, Store> StateDb<DB, Store> {
    pub fn with_fallback(overlay: StateOverlay<Store>, fallback: Option<DB>) -> Self {
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

    pub fn inner(&self) -> &Store {
        self.overlay.inner()
    }

    pub fn inner_mut(&mut self) -> &mut Store {
        self.overlay.inner_mut()
    }
}

impl<DB, Store: OverlayWrite> StateDb<DB, Store> {
    /// Recover the owned EVM state.
    pub fn into_inner(self) -> EvmState {
        self.overlay.into_inner()
    }
}
