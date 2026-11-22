//! The sines: `Sine` by phase accumulation, `FSinOsc` and `PSinGrain` by
//! the two-pole recurrence, and `SinOscFB`, a sine modulated by itself.

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

/// scsynth's `FSinOsc(freq, iphase)`: a sine by the two-pole recurrence
/// `y[n] = 2 cos(w) y[n-1] - y[n-2]`, one multiply and one subtract a sample.
/// `iphase` is the initial phase in cycles (scsynth's is in radians), read at
/// the start.
///
/// The frequency is taken once a block, as scsynth takes it, and a change
/// changes the recurrence's coefficient while its state stays: the sine
/// carries on from where it is, but its amplitude moves with the frequency,
/// which is the price of the recurrence and the reason `Sine` is the
/// oscillator to modulate. A fixed tone is what this is for.
pub struct FSinOsc {
    y1: f64,
    y2: f64,
    b1: f64,
    freq: f32,
    started: bool,
}

impl FSinOsc {
    pub fn new() -> Self {
        Self {
            y1: 0.0,
            y2: 0.0,
            b1: 0.0,
            freq: 0.0,
            started: false,
        }
    }
}

impl Default for FSinOsc {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for FSinOsc {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let rad = TAU / ctx.sample_rate as f64;
        let freq = inputs[0][0];
        if !self.started {
            self.started = true;
            let p = TAU * at(inputs[1], 0) as f64;
            let w = freq as f64 * rad;
            self.freq = freq;
            self.b1 = 2.0 * w.cos();
            self.y1 = (p - w).sin();
            self.y2 = (p - 2.0 * w).sin();
        } else if freq != self.freq {
            self.freq = freq;
            self.b1 = 2.0 * (freq as f64 * rad).cos();
        }
        let (b1, mut y1, mut y2) = (self.b1, self.y1, self.y2);
        for s in output.iter_mut() {
            let y0 = b1 * y1 - y2;
            *s = y0 as f32;
            y2 = y1;
            y1 = y0;
        }
        self.y1 = y1;
        self.y2 = y2;
    }
}

/// scsynth's `SinOscFB(freq, feedback)`: a sine phase-modulated by its own
/// last sample, `y[n] = sin(phase + feedback * y[n-1])`, `feedback` in
/// radians -- from a sine at 0 towards a sawtooth near 1.5 and noise beyond.
/// Computed rather than read from scsynth's table; a control-rate `feedback`
/// is a line across the block, as scsynth's is.
pub struct SinOscFb {
    phase: f64,
    prev: f64,
    feedback: Ramp,
}

impl SinOscFb {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            prev: 0.0,
            feedback: Ramp::default(),
        }
    }
}

impl Default for SinOscFb {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for SinOscFb {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let dt = 1.0 / ctx.sample_rate as f64;
        let fb = inputs[1];
        let (mut k, step) = if fb.len() == 1 {
            self.feedback.toward(fb[0] as f64, output.len())
        } else {
            (0.0, 0.0)
        };
        for (i, s) in output.iter_mut().enumerate() {
            let f = if fb.len() == 1 { k } else { fb[i] as f64 };
            self.prev = (TAU * self.phase + f * self.prev).sin();
            *s = self.prev as f32;
            k += step;
            self.phase += at(inputs[0], i) as f64 * dt;
            self.phase -= self.phase.floor();
        }
    }
}

/// scsynth's `PSinGrain(freq, dur, amp)`: one sine grain of `dur` seconds
/// under a parabolic envelope peaking at `amp` in its middle, its three
/// inputs read at the start. The synth is freed when the grain ends, as
/// scsynth's frees it: a grain is a synth's whole life.
pub struct PSinGrain {
    y1: f64,
    y2: f64,
    b1: f64,
    level: f64,
    slope: f64,
    curve: f64,
    counter: i64,
    started: bool,
}

impl PSinGrain {
    pub fn new() -> Self {
        Self {
            y1: 0.0,
            y2: 0.0,
            b1: 0.0,
            level: 0.0,
            slope: 0.0,
            curve: 0.0,
            counter: 0,
            started: false,
        }
    }
}

impl Default for PSinGrain {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for PSinGrain {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        if !self.started {
            self.started = true;
            let (freq, dur, amp) = (
                inputs[0][0] as f64,
                inputs[1][0] as f64,
                inputs[2][0] as f64,
            );
            let w = freq * TAU / sr;
            let samples = sr * dur;
            let r = 1.0 / samples;
            // The envelope is the parabola 4 (n/N)(1 - n/N), stepped by its
            // first and second differences.
            self.slope = 4.0 * (r - r * r);
            self.curve = -8.0 * r * r;
            self.counter = (samples + 0.5) as i64;
            self.b1 = 2.0 * w.cos();
            self.y1 = -w.sin() * amp;
            self.y2 = -(2.0 * w).sin() * amp;
        }
        for s in output.iter_mut() {
            if self.counter <= 0 {
                *s = 0.0;
                continue;
            }
            let y0 = self.b1 * self.y1 - self.y2;
            self.y2 = self.y1;
            self.y1 = y0;
            *s = (y0 * self.level) as f32;
            self.level += self.slope;
            self.slope += self.curve;
            self.counter -= 1;
        }
    }

    fn done(&self) -> crate::dsp::DoneAction {
        if self.started && self.counter <= 0 {
            crate::dsp::DoneAction::FreeSelf
        } else {
            crate::dsp::DoneAction::None
        }
    }

    fn is_done(&self) -> bool {
        self.started && self.counter <= 0
    }
}
