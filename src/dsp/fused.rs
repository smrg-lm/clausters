//! The fused arithmetic UGens scsynth optimizes: `MulAdd` (`a*b + c`) and
//! `Sum3`/`Sum4` (three/four-operand sums). They are ordinary block processors
//! — one entry each in the registry — whose per-sample math goes through the
//! shared `clausters_core::builtins` operators, so a client folding the same
//! expression off the RT path matches them to the bit.

use clausters_core::builtins;

use crate::dsp::{ProcessCtx, UGen};

/// `a*b + c` (inputs 0, 1, 2) in one UGen — the multiply-accumulate scsynth
/// fuses. Computed as `add(mul(a, b), c)` with the core operators.
pub struct MulAdd;

impl UGen for MulAdd {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        builtins::mul_add_slice(inputs[0], inputs[1], inputs[2], output);
    }
}

/// `a + b + c` (inputs 0, 1, 2) — the three-operand sum, added left to right.
pub struct Sum3;

impl UGen for Sum3 {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        builtins::sum3_slice(inputs[0], inputs[1], inputs[2], output);
    }
}

/// `a + b + c + d` (inputs 0..4) — the four-operand sum, added left to right.
pub struct Sum4;

impl UGen for Sum4 {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        builtins::sum4_slice(inputs[0], inputs[1], inputs[2], inputs[3], output);
    }
}
