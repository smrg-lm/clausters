use std::f64::consts::TAU;

use crate::dsp::{ProcessCtx, UGen, at};

/// Sine oscillator by phase accumulation. Input 0: frequency in Hz (signal or
/// constant).
///
/// The phase is `f64` to match the rest of the phase family rather than because
/// this row needs it: wrapped at `TAU` it never grows, and a wrapped `f32` phase
/// measures the same pitch after ten seconds (see [`phase`](super::phase) for
/// the figures and for the row that does need the precision).
pub struct Sine {
    phase: f64,
}

impl Sine {
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }
}

impl Default for Sine {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Sine {
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
