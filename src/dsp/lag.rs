//! One-pole smoothers: `Lag` (symmetric) and `VarLag` (separate up/down
//! times). These are the single lag implementation the typed controls reuse
//! — a lagged control compiles to an inserted `Lag`/`VarLag` UGen rather than a
//! bespoke control path (see `synthdef::compile`), so client-authored `Lag`
//! and control smoothing share exactly this DSP.
//!
//! The coefficient is scsynth's: `b1 = exp(ln(0.001) / (time · sampleRate))`,
//! so `time` is the -60 dB convergence time in seconds; `time <= 0` passes the
//! input straight through. The state is primed to the first input sample, so a
//! synth does not glide up from zero when it starts.

use crate::dsp::{ProcessCtx, UGen, at};

/// `ln(0.001)` — the smoother converges to within -60 dB over `time` seconds.
const LOG001: f32 = -6.907_755_4;

#[inline]
fn coeff(time: f32, sample_rate: f32) -> f32 {
    if time <= 0.0 || sample_rate <= 0.0 {
        0.0
    } else {
        (LOG001 / (time * sample_rate)).exp()
    }
}

/// `Lag(in, time)`: a one-pole lowpass smoothing `in` over `time` seconds
/// (symmetric). Inputs 0 `in`, 1 `time`.
pub struct Lag {
    y: f32,
    primed: bool,
}

impl Lag {
    pub fn new() -> Self {
        Self {
            y: 0.0,
            primed: false,
        }
    }
}

impl Default for Lag {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Lag {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            if !self.primed {
                self.y = x;
                self.primed = true;
            }
            let b1 = coeff(at(inputs[1], i), ctx.sample_rate);
            self.y = x + b1 * (self.y - x);
            *out = self.y;
        }
    }
}

/// `VarLag(in, lagUp, lagDown)`: a one-pole smoother with separate rise and
/// fall times — `lagUp` while the input is above the state, `lagDown` while
/// below. Inputs 0 `in`, 1 `lagUp`, 2 `lagDown`.
pub struct VarLag {
    y: f32,
    primed: bool,
}

impl VarLag {
    pub fn new() -> Self {
        Self {
            y: 0.0,
            primed: false,
        }
    }
}

impl Default for VarLag {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for VarLag {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            if !self.primed {
                self.y = x;
                self.primed = true;
            }
            let time = if x >= self.y {
                at(inputs[1], i)
            } else {
                at(inputs[2], i)
            };
            let b1 = coeff(time, ctx.sample_rate);
            self.y = x + b1 * (self.y - x);
            *out = self.y;
        }
    }
}
