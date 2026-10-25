use super::super::{CallInput, CallOutput};
use super::CallHandler;
use crate::execution::evm::CallHookError;
use crate::execution::evm::tracking::ReadRecorder;
use alloy_evm::EvmInternals;
use alloy_sol_types::{SolInterface, sol};

sol! {
    interface IScalar {
        function init(uint256 min, uint256 max, uint256 initial) external returns (address handle);
        function add(address handle, int256 delta) external returns (bool);
        function get(address handle) external view returns (uint256);
    }
}

pub struct Scalar {
    read_recorder: ReadRecorder,
}

impl Scalar {
    pub fn new(read_recorder: ReadRecorder) -> Self {
        Self { read_recorder }
    }

    /// Handles scalar initialization; state logic is not implemented yet.
    fn init(
        &self,
        _call: IScalar::initCall,
        _input: &CallInput<'_>,
        _internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError> {
        Ok(CallOutput::reverted(0, Vec::new()))

        // write to Journal
        // but
    }

    /// Handles scalar additions; state logic is not implemented yet.
    fn add(
        &self,
        _call: IScalar::addCall,
        _input: &CallInput<'_>,
        _internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError> {
        Ok(CallOutput::reverted(0, Vec::new()))
    }

    /// Handles scalar reads; state logic is not implemented yet.
    fn get(
        &self,
        _call: IScalar::getCall,
        _input: &CallInput<'_>,
        _internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError> {
        Ok(CallOutput::reverted(0, Vec::new()))
    }
}

impl CallHandler for Scalar {
    fn call(
        &self,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
    ) -> Result<CallOutput, CallHookError> {
        // Decode calldata once, then dispatch to the matching API method.
        match IScalar::IScalarCalls::abi_decode_validate(input.input) {
            Ok(IScalar::IScalarCalls::init(call)) => self.init(call, input, internals),
            Ok(IScalar::IScalarCalls::add(call)) => self.add(call, input, internals),
            Ok(IScalar::IScalarCalls::get(call)) => self.get(call, input, internals),
            Err(_) => Ok(CallOutput::reverted(0, Vec::new())),
        }
    }
}
