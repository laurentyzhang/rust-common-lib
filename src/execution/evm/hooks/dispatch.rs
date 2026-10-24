//! Translation between Alloy/REVM's context-aware precompile boundary and
//! Arcology's call hook contract.

use super::super::CallHookError;
use super::{CallInput, CallRouter, CallScheme, ExitStatus};
use alloy_evm::{
    precompiles::PrecompileInput,
    revm::precompile::{PrecompileError, PrecompileHalt, PrecompileOutput, PrecompileResult},
};

/// Dispatches a REVM call frame through the Arcology hook router.
///
/// This function is installed as a stateful, non-cacheable precompile by
/// [`ArcologyEvmFactory`](super::super::ArcologyEvmFactory). The `PrecompileInput`
/// carries the journal-backed `EvmInternals` passed to stateful call hooks.
pub fn dispatch_precompile(
    router: &CallRouter,
    mut input: PrecompileInput<'_>,
) -> PrecompileResult {
    let scheme = if input.is_static_call() {
        CallScheme::StaticCall
    } else if input.is_direct_call() {
        CallScheme::Call
    } else {
        CallScheme::CallCodeOrDelegateCall
    };

    let hook_input = CallInput {
        caller: input.caller,
        target: input.target_address,
        code_address: input.bytecode_address,
        value: input.value,
        input: input.data,
        gas_limit: input.gas,
        scheme,
    };

    match router.dispatch(&hook_input, &mut input.internals) {
        Ok(Some(output)) => match output.status {
            ExitStatus::Return => Ok(PrecompileOutput::new(
                output.gas_used,
                output.output,
                input.reservoir,
            )),
            ExitStatus::Revert => Ok(PrecompileOutput::revert(
                output.gas_used,
                output.output,
                input.reservoir,
            )),
        },
        Ok(None) => Err(PrecompileError::Fatal(format!(
            "Arcology precompile at {:?} has no registered call hook",
            input.bytecode_address
        ))),
        Err(CallHookError::OutOfGas { .. }) => Ok(PrecompileOutput::halt(
            PrecompileHalt::OutOfGas,
            input.reservoir,
        )),
        Err(CallHookError::Handler(message)) => Err(PrecompileError::Fatal(message)),
    }
}
