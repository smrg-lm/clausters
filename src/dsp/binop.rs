use crate::dsp::{ProcessCtx, UGen, at};

/// Binary operator between two signals/constants: inputs 0 and 1.
#[derive(Clone, Copy)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

pub struct BinaryOp {
    op: BinOp,
}

impl BinaryOp {
    pub fn new(op: BinOp) -> Self {
        Self { op }
    }
}

impl UGen for BinaryOp {
    fn process(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (a, b) = (inputs[0], inputs[1]);
        for (i, s) in output.iter_mut().enumerate() {
            let (x, y) = (at(a, i), at(b, i));
            *s = match self.op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                BinOp::Div => x / y,
            };
        }
    }
}
