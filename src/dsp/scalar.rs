//! Scalar / initial-rate (`ir`) UGens: computed once at synth init and
//! held for the node's life. They are the concrete proof of the `ir` init
//! pass in [`crate::synthdef::instance`]: the synth runs each `ir` UGen on the
//! very first block and then never again, so a value that would *differ* if
//! recomputed (like [`Rand`]) stays frozen — that freeze is the whole point of
//! the rate.

use std::sync::atomic::{AtomicU64, Ordering};

use clausters_core::rng;

use crate::dsp::{ProcessCtx, UGen};

/// `SampleRate.ir`: the engine's sample rate in Hz. No inputs. Idempotent
/// (the same every block), so it is the gentle textbook `ir` example.
///
/// This is one of the two places that read the **engine's** rate rather than
/// the running UGen's: the answer is a hardware fact, so `SampleRate.kr`
/// reports the audio rate, not the control rate it is itself running at.
pub struct SampleRate;

impl UGen for SampleRate {
    fn process(&mut self, ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(ctx.full_sample_rate);
    }
}

/// Seeds successive `Rand` instances differently (same discipline as
/// [`crate::dsp::noise::WhiteNoise`]).
static SEED: AtomicU64 = AtomicU64::new(0x2545_F491_4F6C_DD1D);

/// `Rand.ir(lo, hi)`: one uniform random value in `[lo, hi)`, drawn the first
/// time the synth runs (the `ir` init pass) and held forever after. Inputs
/// 0 `lo`, 1 `hi`. Unlike [`SampleRate`], recomputing it would give a *new*
/// number every block, so it only stays constant because the init pass runs it
/// exactly once — making it the sharpest test of that pass.
pub struct Rand {
    noise: rng::WhiteNoise,
}

impl Rand {
    pub fn new() -> Self {
        let seed = SEED.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
        Self {
            noise: rng::WhiteNoise::from_seed(seed),
        }
    }
}

impl Default for Rand {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Rand {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let lo = inputs[0][0];
        let hi = inputs[1][0];
        // next_sample() is in [-1, 1); fold to [0, 1) then to [lo, hi).
        let u = (self.noise.next_sample() * 0.5 + 0.5).clamp(0.0, 1.0);
        output.fill(lo + (hi - lo) * u);
    }
}
