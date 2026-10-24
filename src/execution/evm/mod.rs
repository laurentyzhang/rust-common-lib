//! Arcology-specific hooks used by an EVM adapter.
//!
//! Call metadata is exposed through [`CallInput`]; stateful hooks also receive
//! Alloy's journal-backed `EvmInternals` for transactional state access.

mod error;
mod executor;
mod hooks;
mod op;
mod outcome;
mod state;
mod tracking;

#[cfg(test)]
mod tests;

pub use error::{AccountDecodeError, CallHookError, PrecompileCollisionError, RegisterHookError};
pub use executor::{ArcologyParaBlockExecutor, ParaBlockConfig};
pub use hooks::{
    ApiAddress, CallHandler, CallInput, CallOutput, CallRouter, CallScheme, ExitStatus, Scalar,
    SlotPrefixer, Type, arcology_address, dispatch_precompile,
};
#[doc(hidden)]
pub use op::backend;
pub use op::{ArcologyBlockExecutorFactory, ArcologyEvm, ArcologyEvmFactory, ArcologyTx};
pub use outcome::{ScheduledTransactionResult, TransactionOutcome};
pub use state::key;
pub use state::{AccountInfoCodec, EvmStateCache, EvmStateCommitter, MergeableEvmState};
pub use tracking::inspector;
pub use tracking::{StateReads, delta_records};
