//! `Clip`, `Wrap` and `Fold`: an input kept inside `lo..hi`, the two-bound
//! form of the `clip2`/`wrap2`/`fold2` operators.
//!
//! Three kinds rather than one generic one because that is how a def names
//! them (`Clip.ar(in, lo, hi)`); the arithmetic is one function in the shared
//! core, `clausters_core::builtins::apply_bound`, which a client computes a
//! value with too, so a signal clipped here and a number clipped in a script
//! land in the same place.

use clausters_core::builtins::{BoundOp, bound_slice};

use crate::dsp::{ProcessCtx, UGen};

/// `Clip`/`Wrap`/`Fold(in, lo, hi)`. The bounds are ordinary inputs, so a
/// range may be modulated sample by sample.
pub struct Bound {
    op: BoundOp,
}

impl Bound {
    pub fn new(op: BoundOp) -> Self {
        Self { op }
    }
}

impl UGen for Bound {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        bound_slice(self.op, inputs[0], inputs[1], inputs[2], output);
    }
}
