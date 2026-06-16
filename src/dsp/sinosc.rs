use std::f64::consts::TAU;

use crate::dsp::{ProcessCtx, UGen, at};

/// Sine oscillator by phase accumulation. Input 0: frequency in Hz (signal or
/// constant). The phase is kept in `f64` so the tuning does not degrade over
/// long sessions.
pub struct SinOsc {
    phase: f64,
}

impl SinOsc {
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }
}

impl Default for SinOsc {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for SinOsc {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let freq = inputs[0];
        let sr = ctx.sample_rate as f64;
        for (i, s) in output.iter_mut().enumerate() {
            *s = self.phase.sin() as f32;
            self.phase += TAU * at(freq, i) as f64 / sr;
            if self.phase >= TAU {
                self.phase -= TAU;
            }
        }
    }
}
