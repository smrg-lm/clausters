use clausters_core::builtins::{self, BinaryOp as CoreBinaryOp};

use crate::dsp::{ProcessCtx, UGen};

/// Binary operator between two signals/constants: inputs 0 and 1. The four
/// arithmetic ops are the shared `clausters_core::builtins` operators, so the
/// audio thread and every client compute them with the same code.
#[derive(Clone, Copy, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl BinOp {
    #[inline]
    fn core(self) -> CoreBinaryOp {
        match self {
            BinOp::Add => CoreBinaryOp::Add,
            BinOp::Sub => CoreBinaryOp::Sub,
            BinOp::Mul => CoreBinaryOp::Mul,
            BinOp::Div => CoreBinaryOp::Div,
        }
    }
}

pub struct BinaryOp {
    op: CoreBinaryOp,
}

impl BinaryOp {
    pub fn new(op: BinOp) -> Self {
        Self { op: op.core() }
    }
}

impl UGen for BinaryOp {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        // Broadcasting slice op (allocation-free): identical to the previous
        // hand-written loop, now the single source of truth in the core.
        builtins::binary_slice(self.op, inputs[0], inputs[1], output);
    }
}
