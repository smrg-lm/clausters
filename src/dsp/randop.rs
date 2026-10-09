//! `RandOpUGen`: a random draw per sample, one generic kind carrying the
//! distribution by name the way `RangeMapUGen` carries a map -- sclang's
//! `sig.rand`, `sig.rrand(hi)`, `sig.coin` and their neighbours on a signal.
//!
//! **The distributions are not here.** `clausters_core::rng::Draw` holds them,
//! and a client draws a value with the same function; this is the plumbing
//! that lets a signal through it. Unlike every other operator kind it has
//! state: its own generator, seeded off the graph's seed walk like any other
//! stochastic UGen, so two draws in one graph are two streams.

use clausters_core::rng::{Draw, Rng};

use crate::dsp::{ProcessCtx, UGen, at};

/// `RandOpUGen(a[, b])` with the distribution as a static field: `a` is the
/// signal the draw is scaled by (or the low bound), `b` the high bound of the
/// two-bound draws.
pub struct RandOp {
    draw: Draw,
    rng: Rng,
}

impl RandOp {
    /// The distribution named by the compiler-resolved index, drawing from
    /// `seed`. An index outside the table cannot reach here -- the compiler
    /// resolves the name -- so an unknown one falls back to the first.
    pub fn new(draw: u32, seed: u64) -> Self {
        Self {
            draw: Draw::from_u32(draw).unwrap_or(Draw::Rand),
            rng: Rng::from_seed(seed),
        }
    }
}

impl UGen for RandOp {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, s) in output.iter_mut().enumerate() {
            let a = at(inputs[0], i) as f64;
            let b = at(inputs[1], i) as f64;
            *s = self.rng.draw(self.draw, a, b) as f32;
        }
    }
}
