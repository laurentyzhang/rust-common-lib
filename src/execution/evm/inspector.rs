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

pub enum Reads {
    Balance(Key),
    SelfBalance(Key),
}

#[derive(Default)]
pub struct StateReadRecorder {
    pub ops: Vec<Reads>,
}

impl<CTX> Inspector<CTX> for StateReadRecorder {
    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        match interp.bytecode.opcode() {
            opcode::BALANCE => {
                if let Ok(word) = interp.stack.peek(0) {
                    self.ops
                        .push(Reads::Balance(Key::Account(word.into_address())));
                }
            }

            opcode::SELFBALANCE => {
                self.ops.push(Reads::SelfBalance(Key::Account(
                    interp.input.target_address(),
                )));
            }
            _ => {}
        }
    }
}
