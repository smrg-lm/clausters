//! Scalar / initial-rate (`ir`) UGens: computed once at synth init and
//! held for the node's life. They are the concrete proof of the `ir` init
//! pass in [`crate::synthdef::instance`]: the synth runs each `ir` UGen on the
//! very first block and then never again, so a value that would *differ* if
//! recomputed (like `Rand`, [`crate::dsp::random::Once`]) stays frozen -- that freeze is the whole point of
//! the rate.

use crate::dsp::{ProcessCtx, UGen};

/// `SampleRate.ir`: the engine's sample rate in Hz. No inputs. Idempotent
/// (the same every block), so it is the gentle textbook `ir` example.
///
/// This is one of the two places that read the **engine's** rate rather than
/// the running UGen's: the answer is a hardware fact, so `SampleRate.kr`
/// reports the audio rate, not the control rate it is itself running at.
pub struct SampleRate;

impl UGen for SampleRate {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(ctx.full_sample_rate);
    }
}
