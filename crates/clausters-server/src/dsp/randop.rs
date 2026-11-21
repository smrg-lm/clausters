//! `RandOpUGen`: a random draw per sample, one generic kind carrying the
//! distribution by name the way `RangeMapUGen` carries a map -- sclang's
//! `sig.rand`, `sig.rrand(hi)`, `sig.coin` and their neighbours on a signal.
//!
//! **The distributions are not here.** `clausters_core::rng::Draw` holds them,
//! and a client draws a value with the same function; this is the plumbing
//! that lets a signal through it. Unlike every other operator kind it draws:
//! from the synth's generator, like every random UGen ([`crate::dsp::rand`]).
//! The draws scsynth's operators also have (`rand`, `rand2`, `linrand`,
//! `bilinrand`, `sum3rand`, `coin`, `exprand`) are scsynth's, so the same seed
//! gives scsynth's values; the rest read the generator's uniform through the
//! shared table of distributions. `rrand` is the one exception: scsynth's
//! operator scales a bipolar draw, so it reaches below `lo` by the width of
//! the range, where sclang's `rrand` -- and this, and the client's -- is
//! uniform between the two.

use clausters_core::rng::Draw;

use crate::dsp::rand::SynthRand;
use crate::dsp::{ProcessCtx, UGen, at};

/// `RandOpUGen(a[, b])` with the distribution as a static field: `a` is the
/// signal the draw is scaled by (or the low bound), `b` the high bound of the
/// two-bound draws.
pub struct RandOp {
    draw: Draw,
    rand: SynthRand,
}

impl RandOp {
    /// The distribution named by the compiler-resolved index, drawing from
    /// the synth's generator. An index outside the table cannot reach here --
    /// the compiler resolves the name -- so an unknown one falls back to the
    /// first.
    pub fn new(draw: u32, rand: SynthRand) -> Self {
        Self {
            draw: Draw::from_u32(draw).unwrap_or(Draw::Rand),
            rand,
        }
    }
}

impl UGen for RandOp {
    fn reset(&mut self) {
        // The generator's stream goes on: a restarted voice draws new numbers.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let d = self.draw;
        self.rand.draw(|g| {
            for (i, s) in output.iter_mut().enumerate() {
                let (a, b) = (at(inputs[0], i), at(inputs[1], i));
                // scsynth's `UnaryOpUGen`/`BinaryOpUGen` random operators.
                *s = match d {
                    Draw::Rand => g.frand() * a,
                    Draw::Rand2 => g.frand2() * a,
                    Draw::Linrand => g.flinrand() * a,
                    Draw::Bilinrand => g.fbilinrand() * a,
                    Draw::Sum3rand => g.fsum3rand() * a,
                    Draw::Coin => (g.frand() < a) as u8 as f32,
                    Draw::Exprand => g.exprandrng(a as f64, b as f64) as f32,
                    _ => d.sample(g, a as f64, b as f64) as f32,
                };
            }
        });
    }
}
