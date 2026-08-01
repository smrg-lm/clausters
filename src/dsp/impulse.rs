use crate::dsp::{ProcessCtx, UGen, at};

/// Band-unlimited impulse train: a single-sample `1.0` every `freq` Hz, `0.0`
/// in between. Input 0 is the frequency in Hz (signal or constant), like
/// SuperCollider's `Impulse`.
///
/// The phase starts "due" so the **first** output sample is always an impulse.
/// Combined with a `/sched_at`'d `/synth_new` — which splits the processing block at
/// the target sample, so the synth's first sample *is* the target — this
/// places one pristine impulse on an exact sample of the clock. A frequency
/// of `0` then emits that single impulse and silence forever after, which is
/// exactly how `examples/clock_recorder.py` marks each scheduled instant.
pub struct Impulse {
    /// Cycles accumulated since the last impulse; fires when it reaches 1.
    phase: f64,
}

impl Impulse {
    pub fn new() -> Self {
        Self { phase: 1.0 } // due immediately: the first sample is an impulse
    }
}

impl Default for Impulse {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Impulse {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let freq = inputs[0];
        let sr = ctx.sample_rate as f64;
        for (i, s) in output.iter_mut().enumerate() {
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                *s = 1.0;
            } else {
                *s = 0.0;
            }
            // Cycles per sample; negative or zero frequency never re-arms.
            self.phase += (at(freq, i) as f64 / sr).max(0.0);
        }
    }
}
