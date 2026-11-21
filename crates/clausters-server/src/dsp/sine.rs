use std::f64::consts::TAU;

use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// Sine oscillator by phase accumulation -- scsynth's `SinOsc`, computed
/// rather than read from a table. Inputs: 0 the frequency in Hz, 1 the
/// **phase offset** in cycles.
///
/// The output is `sin(2 pi (p + offset))`, where `p` integrates the
/// frequency: the offset is a running offset, not an initial phase, so an
/// audio-rate offset is phase modulation. A control-rate offset is
/// interpolated across the slice ([`Ramp`]), as scsynth's is, so moving it
/// does not step. scsynth takes the offset in radians; every phase this crate
/// exposes is in cycles.
///
/// The phase is `f64` to match the rest of the phase family rather than because
/// this row needs it: wrapped at one cycle it never grows, and a wrapped `f32`
/// phase measures the same pitch after ten seconds (see [`phase`](super::phase)
/// for the figures and for the row that does need the precision).
pub struct Sine {
    /// The integrated frequency, in cycles, wrapped to `[0, 1)`.
    phase: f64,
    offset: Ramp,
}

impl Sine {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            offset: Ramp::default(),
        }
    }
}

impl Default for Sine {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Sine {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let freq = inputs[0];
        let offset = inputs.get(1).copied().unwrap_or(&[0.0]);
        let dt = 1.0 / ctx.sample_rate as f64;
        let (mut off, step) = if offset.len() == 1 {
            self.offset.toward(offset[0] as f64, output.len())
        } else {
            (0.0, 0.0)
        };
        for (i, s) in output.iter_mut().enumerate() {
            let o = if offset.len() == 1 {
                off
            } else {
                offset[i] as f64
            };
            *s = (TAU * (self.phase + o)).sin() as f32;
            off += step;
            self.phase += at(freq, i) as f64 * dt;
            self.phase -= self.phase.floor();
        }
    }
}
