use clausters_core::builtins::{self, UnaryOp as CoreUnaryOp};

use crate::dsp::{ProcessCtx, UGen};

/// Unary operator on one signal (input 0): the generic `UnaryOpUGen`, selected
/// by a special-index `op` (a `clausters_core::builtins::UnaryOp` discriminant).
/// The scalar math is the shared core, so the audio thread and every client
/// compute each op with the same code — bit-identical off the RT path.
pub struct UnaryOp {
    op: CoreUnaryOp,
}

impl UnaryOp {
    /// Builds from a core opcode index (the `UnaryOpUGen` wire `op` field).
    /// The compiler validates the index first; an unknown value cannot reach
    /// here and falls back to `Neg` defensively.
    pub fn from_index(op: u32) -> Self {
        Self {
            op: CoreUnaryOp::from_u32(op).unwrap_or(CoreUnaryOp::Neg),
        }
    }
}

impl UGen for UnaryOp {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        builtins::unary_slice(self.op, inputs[0], output);
    }
}
