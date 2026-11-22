//! Banks specified by arrays, scsynth's from its `OscUGens.cpp`: `Klang`, a
//! bank of fixed sines, and `Klank`, a bank of fixed resonators.
//!
//! The specification -- a frequency, an amplitude and a phase or a ring time
//! per partial -- is read at the synth's start and kept, as scsynth keeps
//! it: these are the cheap fixed banks, and the state per partial is
//! allocated when the synth is built, from the number of inputs the def
//! gives the UGen.

use std::f64::consts::TAU;

use crate::dsp::sections::flush;
use crate::dsp::{ProcessCtx, UGen};

/// One partial of a [`Klang`]: the recurrence's state and its coefficient.
#[derive(Clone, Copy, Default)]
struct Partial {
    y1: f64,
    y2: f64,
    b1: f64,
}

/// scsynth's `Klang(freqscale, freqoffset, freq, amp, phase, ...)`: a sum of
/// fixed sines, each `amp * sin(2 pi freq' t + phase)` with `freq'` its
/// frequency times `freqscale` plus `freqoffset`, by the two-pole
/// recurrence `FSinOsc` uses. `phase` is in cycles (scsynth's is in
/// radians).
pub struct Klang {
    partials: Vec<Partial>,
    started: bool,
}

/// Inputs before a `Klang`'s partials.
const KLANG_HEAD: usize = 2;

impl Klang {
    /// A bank for a def that gives it `inputs` inputs.
    pub fn new(inputs: usize) -> Self {
        Self {
            partials: vec![Partial::default(); inputs.saturating_sub(KLANG_HEAD) / 3],
            started: false,
        }
    }
}

impl UGen for Klang {
    fn reset(&mut self) {
        self.started = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.started {
            self.started = true;
            let rad = TAU / ctx.sample_rate as f64;
            let (scale, offset) = (inputs[0][0] as f64, inputs[1][0] as f64);
            for (k, p) in self.partials.iter_mut().enumerate() {
                let j = KLANG_HEAD + 3 * k;
                let w = (inputs[j][0] as f64 * scale + offset) * rad;
                let level = inputs[j + 1][0] as f64;
                let phase = TAU * inputs[j + 2][0] as f64;
                p.y1 = level * (phase - w).sin();
                p.y2 = level * (phase - 2.0 * w).sin();
                p.b1 = 2.0 * w.cos();
            }
        }
        output.fill(0.0);
        for p in self.partials.iter_mut() {
            let (b1, mut y1, mut y2) = (p.b1, p.y1, p.y2);
            for s in output.iter_mut() {
                let y0 = b1 * y1 - y2;
                *s += y0 as f32;
                y2 = y1;
                y1 = y0;
            }
            p.y1 = y1;
            p.y2 = y2;
        }
    }
}

/// One resonator of a [`Klank`].
#[derive(Clone, Copy, Default)]
struct Resonator {
    y1: f64,
    y2: f64,
    b1: f64,
    b2: f64,
    a0: f64,
}

/// Inputs before a `Klank`'s partials: the input, `freqscale`, `freqoffset`
/// and `decayscale`.
const KLANK_HEAD: usize = 4;

/// scsynth's `Klank(input, freqscale, freqoffset, decayscale, freq, amp,
/// time, ...)`: a bank of resonators the input excites, each ringing at its
/// frequency (times `freqscale`, plus `freqoffset`) for `time` seconds (times
/// `decayscale`) to -60 dB, at `amp`.
///
/// Each is scsynth's two-pole section with its poles at radius `R` and the
/// angle that puts the response's **peak** at the frequency rather than the
/// poles there (`cos(theta) = 2R cos(w) / (1 + R^2)`), and the bank's sum
/// passes through zeros at DC and Nyquist, `y[n] - y[n-2]`.
pub struct Klank {
    resonators: Vec<Resonator>,
    /// The bank's sum one and two samples back, for the output's zeros.
    s1: f64,
    s2: f64,
    started: bool,
}

impl Klank {
    /// A bank for a def that gives it `inputs` inputs.
    pub fn new(inputs: usize) -> Self {
        Self {
            resonators: vec![Resonator::default(); inputs.saturating_sub(KLANK_HEAD) / 3],
            s1: 0.0,
            s2: 0.0,
            started: false,
        }
    }
}

impl UGen for Klank {
    fn reset(&mut self) {
        for r in self.resonators.iter_mut() {
            r.y1 = 0.0;
            r.y2 = 0.0;
        }
        self.s1 = 0.0;
        self.s2 = 0.0;
        self.started = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let x = inputs[0];
        if !self.started {
            self.started = true;
            let sr = ctx.sample_rate as f64;
            let rad = TAU / sr;
            let (scale, offset) = (inputs[1][0] as f64, inputs[2][0] as f64);
            let decay = inputs[3][0] as f64;
            for (k, r) in self.resonators.iter_mut().enumerate() {
                let j = KLANK_HEAD + 3 * k;
                let w = (inputs[j][0] as f64 * scale + offset) * rad;
                let time = inputs[j + 2][0] as f64 * decay;
                let radius = if time == 0.0 {
                    0.0
                } else {
                    (0.001f64.ln() / (time * sr)).exp()
                };
                let cos_theta = 2.0 * radius * w.cos() / (1.0 + radius * radius);
                r.b1 = 2.0 * radius * cos_theta;
                r.b2 = -radius * radius;
                r.a0 = inputs[j + 1][0] as f64 * 0.25;
            }
        }
        for (i, s) in output.iter_mut().enumerate() {
            let xi = if x.len() == 1 { x[0] } else { x[i] } as f64;
            let mut sum = 0.0;
            for r in self.resonators.iter_mut() {
                let y0 = xi + r.b1 * r.y1 + r.b2 * r.y2;
                r.y2 = r.y1;
                r.y1 = y0;
                sum += r.a0 * y0;
            }
            *s = (sum - self.s2) as f32;
            self.s2 = self.s1;
            self.s1 = sum;
        }
        for r in self.resonators.iter_mut() {
            r.y1 = flush(r.y1);
            r.y2 = flush(r.y2);
        }
    }
}
