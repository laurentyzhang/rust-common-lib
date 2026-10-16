use crate::execution::evm::key::Key;
use crate::execution::{VmStateCache, cache};
use alloy_evm::revm::{
    Inspector,
    bytecode::opcode,
    interpreter::{
        Interpreter,
        instructions::utility::IntoAddress,
        interpreter_types::{InputsTr, Jumps},
    },
};

pub enum Reads {
    Balance(Key),
    SelfBalance(Key),
}

pub struct StateReadRecorder {
    // pub ops: Vec<Reads>,
    pub state_cache: VmStateCache<'static, Key>,
}

impl StateReadRecorder {
    pub fn new(cache: VmStateCache<'static, Key>) -> Self {
        Self { state_cache: cache }
    }
}

impl<CTX> Inspector<CTX> for StateReadRecorder {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        // The frame's bytecode has been loaded before its interpreter is initialized.
        if let Some(address) = interp.input.bytecode_address() {
            let _ = self.state_cache.get(&Key::Code(*address));
        }
    }

    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        match interp.bytecode.opcode() {
            opcode::BALANCE => {
                if let Ok(word) = interp.stack.peek(0) {
                    // We don't care about the result, just log the read.
                    let _ = self.state_cache.get(&Key::Account(word.into_address()));
                }
            }

            opcode::SELFBALANCE => {
                let _ = self
                    .state_cache
                    .get(&Key::Account(interp.input.target_address()));
            }

            opcode::CODECOPY => {
                if let Some(address) = interp.input.bytecode_address() {
                    let _ = self.state_cache.get(&Key::Code(*address));
                }
            }

            opcode::EXTCODESIZE | opcode::EXTCODECOPY | opcode::EXTCODEHASH => {
                if let Ok(word) = interp.stack.peek(0) {
                    let _ = self
                        .state_cache
                        .get(&Key::Code(word.into_address()));
                }
            }
            _ => {}
        }
    }
}
