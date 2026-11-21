//! The smoothers: the one-pole `Lag` (symmetric) and `LagUD` (separate up and
//! down times), and `VarLag`, a straight line to each new value. The two
//! one-poles are the single lag implementation the typed controls reuse -- a
//! lagged control compiles to an inserted `Lag`/`LagUD` UGen rather than a
//! bespoke control path (see `synthdef::compile`), so client-authored `Lag`
//! and control smoothing share exactly this DSP.
//!
//! The coefficient is scsynth's: `b1 = exp(ln(0.001) / (time * sampleRate))`,
//! so `time` is the -60 dB convergence time in seconds; `time <= 0` passes the
//! input straight through. The state is primed to the first input sample, so a
//! synth does not glide up from zero when it starts.

use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// `ln(0.001)` -- the smoother converges to within -60 dB over `time` seconds.
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
///
/// A control-rate `time` moves the coefficient in a straight line across the
/// slice when it changes ([`Ramp`]), as scsynth's does, so a moving lag time
/// does not step.
pub struct Lag {
    y: f32,
    primed: bool,
    b1: Ramp,
}

impl Lag {
    pub fn new() -> Self {
        Self {
            y: 0.0,
            primed: false,
            b1: Ramp::default(),
        }
    }
}

impl Default for Lag {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Lag {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn resume(&mut self) {
        // Primed again from the first input it sees, which is the value its
        // driver has *now* rather than the one it had when the music stopped.
        self.primed = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let time = inputs[1];
        let (mut b1, step) = if time.len() == 1 {
            let (b, d) = self
                .b1
                .toward(coeff(time[0], ctx.sample_rate) as f64, output.len());
            (b as f32, d as f32)
        } else {
            (0.0, 0.0)
        };
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            if !self.primed {
                self.y = x;
                self.primed = true;
            }
            if time.len() == 1 {
                b1 += step;
            } else {
                b1 = coeff(time[i], ctx.sample_rate);
            }
            self.y = x + b1 * (self.y - x);
            *out = self.y;
        }
        if time.len() > 1 {
            self.b1.set(b1 as f64);
        }
    }
}

/// `LagUD(in, lagUp, lagDown)`: a one-pole smoother with separate rise and
/// fall times -- `lagUp` while the input is above the state, `lagDown` while
/// below. Inputs 0 `in`, 1 `lagUp`, 2 `lagDown`.
///
/// Control-rate times move their coefficients in a line across the slice, as
/// `Lag`'s does.
pub struct LagUD {
    y: f32,
    primed: bool,
    b1: [Ramp; 2],
}

impl LagUD {
    pub fn new() -> Self {
        Self {
            y: 0.0,
            primed: false,
            b1: [Ramp::default(); 2],
        }
    }
}

impl Default for LagUD {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for LagUD {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn resume(&mut self) {
        self.primed = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let n = output.len();
        let sr = ctx.sample_rate;
        // Each time: (current coefficient, per-sample step), or per-sample
        // from an audio-rate time.
        let mut b = [(0.0f32, 0.0f32); 2];
        for (k, slot) in b.iter_mut().enumerate() {
            let t = inputs[1 + k];
            if t.len() == 1 {
                let (v, d) = self.b1[k].toward(coeff(t[0], sr) as f64, n);
                *slot = (v as f32, d as f32);
            }
        }
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            if !self.primed {
                self.y = x;
                self.primed = true;
            }
            for (k, slot) in b.iter_mut().enumerate() {
                let t = inputs[1 + k];
                if t.len() == 1 {
                    slot.0 += slot.1;
                } else {
                    slot.0 = coeff(t[i], sr);
                }
            }
            let b1 = if x > self.y { b[0].0 } else { b[1].0 };
            self.y = x + b1 * (self.y - x);
            *out = self.y;
        }
        for (k, slot) in b.iter().enumerate() {
            if inputs[1 + k].len() > 1 {
                self.b1[k].set(slot.0 as f64);
            }
        }
    }
}

/// `VarLag(in, time, start)`: a **straight line** to each new value of `in`,
/// taking `time` seconds -- scsynth's `VarLag` UGen, ported from its code.
/// Inputs 0 `in`, 1 `time`, 2 `start`, the value the output starts from.
///
/// Where a one-pole approaches a target and never reaches it, this arrives,
/// in `time` and exactly: a new value starts a new line from wherever the
/// output is. Changing `time` partway through rescales what is left of the
/// line, so the output keeps its place and only its speed changes. (sclang's
/// `VarLag` reaches the shaped segments through an `EnvGen`; this is the
/// linear one, which is the UGen.)
pub struct VarLag {
    level: f64,
    slope: f64,
    counter: i64,
    target: f32,
    time: f32,
    started: bool,
}

impl VarLag {
    pub fn new() -> Self {
        Self {
            level: 0.0,
            slope: 0.0,
            counter: 0,
            target: 0.0,
            time: 0.0,
            started: false,
        }
    }

    fn aim(&mut self, x: f32, time: f32, sr: f32) {
        // In `f64`, as scsynth multiplies its `float` time by its `double`
        // rate: which side of a whole count it truncates from is the count.
        self.counter = ((time as f64 * sr as f64) as i64).max(1);
        self.slope = (x as f64 - self.level) / self.counter as f64;
        self.target = x;
        self.time = time;
    }
}

impl Default for VarLag {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for VarLag {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (sig, time) = (inputs[0], inputs[1]);
        let sr = ctx.sample_rate;
        if !self.started {
            self.started = true;
            self.level = at(inputs[2], 0) as f64;
            self.aim(at(sig, 0), at(time, 0), sr);
        }
        for (i, out) in output.iter_mut().enumerate() {
            let (x, t) = (at(sig, i), at(time, i));
            if x != self.target {
                self.aim(x, t, sr);
            } else if t != self.time {
                // A new time rescales what is left of the line.
                if self.counter != 0 && self.time != 0.0 {
                    let scale = t as f64 / self.time as f64;
                    self.counter = ((self.counter as f64 * scale) as i64).max(1);
                    self.slope /= scale;
                }
                self.time = t;
            }
            *out = self.level as f32;
            if self.counter > 0 {
                self.level += self.slope;
                self.counter -= 1;
            } else {
                self.level = self.target as f64;
            }
        }
    }
}
