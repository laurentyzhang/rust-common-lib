//! Arcology-specific hooks used by an EVM adapter.
//!
//! This module deliberately keeps the hook contract independent from a specific
//! REVM release. The Reth/REVM integration only needs to translate its call
//! frame into [`CallHookInput`] and translate [`CallHookOutcome`] back.

mod executor;
mod hooks;
mod op;
mod outcome;
mod state;
mod tracking;

#[cfg(test)]
mod tests;

pub use executor::{ArcologyParaBlockExecutor, ParaBlockConfig};
pub use hooks::{
    ArcologyApi, CallHook, CallHookError, CallHookInput, CallHookOutcome, CallHookOutput,
    CallHookRouter, CallScheme, ClosureCallHook, ExitStatus, RegisterHookError, arcology_address,
    dispatch_precompile,
};
#[doc(hidden)]
pub use op::backend;
pub use op::{
    ArcologyBlockExecutorFactory, ArcologyEvm, ArcologyEvmFactory, ArcologyTx,
    PrecompileCollisionError,
};
pub use outcome::{ScheduledTransactionResult, TransactionOutcome};
pub use state::key;
pub use state::{AccountDecodeError, AccountInfoCodec, EvmStateCache, MergeableEvmState, StateDB};
pub use tracking::inspector;
pub use tracking::{StateReads, delta_records};
// pub use key::Key;
