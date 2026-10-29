//! Opcode-based read capture backed by the shared state read recorder.

use super::recorder::ReadRecorder;
use crate::execution::evm::key::Key;
use alloy_evm::revm::{
    Inspector,
    bytecode::opcode,
    interpreter::{
        Interpreter,
        instructions::utility::IntoAddress,
        interpreter_types::{InputsTr, Jumps},
    },
};

impl<CTX> Inspector<CTX> for ReadRecorder {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        // The frame's bytecode has been loaded before its interpreter is initialized.
        if let Some(address) = interp.input.bytecode_address() {
            self.record(Key::Code(*address));
        }
    }

    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        match interp.bytecode.opcode() {
            opcode::BALANCE => {
                if let Ok(word) = interp.stack.peek(0) {
                    // We don't care about the result, just log the read.
                    self.record(Key::Balance(word.into_address()));
                }
            }

            opcode::SELFBALANCE => {
                // We don't care about the result, just log the read.
                self.record(Key::Balance(interp.input.target_address()));
            }

            opcode::SLOAD => {
                if let Ok(slot) = interp.stack.peek(0) {
                    self.record(Key::Storage(interp.input.target_address(), slot));
                }
            }

            opcode::CODECOPY => {
                if let Some(address) = interp.input.bytecode_address() {
                    self.record(Key::Code(*address));
                }
            }

            opcode::EXTCODESIZE | opcode::EXTCODECOPY | opcode::EXTCODEHASH => {
                if let Ok(word) = interp.stack.peek(0) {
                    self.record(Key::Code(word.into_address()));
                }
            }
            _ => {}
        }
    }
}
