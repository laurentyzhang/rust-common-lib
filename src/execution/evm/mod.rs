//! Arcology-specific hooks used by an EVM adapter.
//!
//! This module deliberately keeps the hook contract independent from a specific
//! REVM release. The Reth/REVM integration only needs to translate its call
//! frame into [`CallHookInput`] and translate [`CallHookOutcome`] back.

mod account_info;
#[doc(hidden)]
pub mod backend;
mod call_router;
mod dispatch;
mod executor;
mod factory;
pub mod inspector;
pub mod key;
mod outcome;
mod result_and_state;
mod statedb;
mod transaction;

#[cfg(test)]
mod tests;

pub use account_info::{AccountDecodeError, AccountInfoCodec};
pub use backend::ArcologyBlockExecutorFactory;
pub use call_router::{
    ArcologyApi, CallHook, CallHookError, CallHookInput, CallHookOutcome, CallHookOutput,
    CallHookRouter, CallScheme, ClosureCallHook, ExitStatus, RegisterHookError, arcology_address,
};
pub use dispatch::dispatch_precompile;
pub use executor::{ArcologyParaBlockExecutor, ParaBlockConfig, ScheduledTransactionResult};
pub use factory::{ArcologyEvm, ArcologyEvmFactory, PrecompileCollisionError};
pub use inspector::StateReads;
pub use outcome::TransactionOutcome;
// pub use key::Key;
pub use result_and_state::MergeableResultAndState;
pub use statedb::StateDB;
pub use transaction::ArcologyTx;
