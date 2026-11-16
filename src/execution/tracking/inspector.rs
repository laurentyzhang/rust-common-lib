//! Opcode reads, writes, and deletion attempts captured by the shared recorder.

use super::recorder::Recorder;
use crate::execution::key::Key;
use alloy_evm::revm::{
    Inspector,
    bytecode::opcode,
    interpreter::{
        Interpreter,
        instructions::utility::IntoAddress,
        interpreter_types::{InputsTr, Jumps},
    },
};

impl<CTX> Inspector<CTX> for Recorder {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        // The frame's bytecode has been loaded before its interpreter is initialized.
        if let Some(address) = interp.input.bytecode_address() {
            self.log_read(Key::Code(*address));
        }
    }

    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        match interp.bytecode.opcode() {
            opcode::BALANCE => {
                if let Ok(word) = interp.stack.peek(0) {
                    // We don't care about the result, just log the read.
                    self.log_read(Key::Balance(word.into_address()));
                }
            }

            opcode::SELFBALANCE => {
                // We don't care about the result, just log the read.
                self.log_read(Key::Balance(interp.input.target_address()));
            }

            opcode::SLOAD => {
                if let Ok(slot) = interp.stack.peek(0) {
                    self.log_read(Key::Storage(interp.input.target_address(), slot));
                }
            }

            opcode::SSTORE => {
                if let (Ok(slot), Ok(value)) = (interp.stack.peek(0), interp.stack.peek(1)) {
                    let key = Key::Storage(interp.input.target_address(), slot);
                    if value.is_zero() {
                        self.log_delete(key);
                    } else {
                        self.log_write(key);
                    }
                }
            }

            opcode::CODECOPY => {
                if let Some(address) = interp.input.bytecode_address() {
                    self.log_read(Key::Code(*address));
                }
            }

            opcode::EXTCODESIZE | opcode::EXTCODECOPY | opcode::EXTCODEHASH => {
                if let Ok(word) = interp.stack.peek(0) {
                    self.log_read(Key::Code(word.into_address()));
                }
            }
            _ => {}
        }
    }
}
