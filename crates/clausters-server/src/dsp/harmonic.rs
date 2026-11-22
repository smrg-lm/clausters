//! Harmonic spectra in closed form, scsynth's from its `OscUGens.cpp`:
//! `Blip`, a band-limited impulse train, and `Formant`, a formant over a
//! fundamental.
//!
//! Both are computed rather than read from scsynth's sine and cosecant
//! tables, so what they give is the formula the tables approximate.

use std::f64::consts::{PI, TAU};

use crate::dsp::{ProcessCtx, UGen};

/// scsynth's `Blip(freq, numharm)`: `numharm` cosine harmonics of `freq` of
/// equal amplitude, summed and scaled by `1/numharm` so the peak is 1 -- the
/// band-limited impulse train, in closed form as the Dirichlet kernel,
/// `(sin((2N + 1) x) / sin(x) - 1) / 2N` with `x` half the phase.
///
/// The count is kept below Nyquist (`N <= sr / 2 / freq`, at least 1), and a
/// change of it crossfades the two kernels across the block, as scsynth's
/// does. Both inputs are read once a block, as scsynth reads them. scsynth
/// advances the phase at the larger of the last and the new frequency in a
/// block where the count is clipped; here it advances at the frequency.
pub struct Blip {
    /// The fundamental's phase, in cycles; the kernel reads half of it.
    phase: f64,
    n: i32,
    freq: f32,
    numharm: i32,
    started: bool,
}

impl Blip {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            n: 1,
            freq: 0.0,
            numharm: 0,
            started: false,
        }
    }

    /// The harmonics for `numharm` at `freq`: below Nyquist, at least one.
    fn count(numharm: i32, freq: f32, sr: f32) -> i32 {
        let max = (sr as f64 * 0.5 / freq as f64) as i32;
        numharm.min(max).max(1)
    }

    /// The kernel with `n` harmonics at `x = pi * phase`.
    #[inline]
    fn kernel(n: i32, x: f64) -> f64 {
        let den = x.sin();
        if den.abs() < 1e-9 {
            // The limit: every harmonic at its peak.
            1.0
        } else {
            ((((2 * n + 1) as f64) * x).sin() / den - 1.0) * 0.5 / n as f64
        }
    }
}

impl Default for Blip {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Blip {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate;
        let freq = inputs[0][0];
        let numharm = inputs[1][0] as i32;
        if !self.started {
            self.started = true;
            self.freq = freq;
            self.numharm = numharm;
            self.n = Self::count(numharm, freq, sr);
        }
        let prev = self.n;
        if numharm != self.numharm || freq != self.freq {
            self.n = Self::count(numharm, freq, sr);
            self.numharm = numharm;
            self.freq = freq;
        }
        let dt = freq as f64 / sr as f64;
        let n = self.n;
        let fade = 1.0 / output.len() as f64;
        for (i, s) in output.iter_mut().enumerate() {
            let x = PI * self.phase;
            let y = if n == prev {
                Self::kernel(n, x)
            } else {
                let a = Self::kernel(prev, x);
                a + (Self::kernel(n, x) - a) * (i as f64 * fade)
            };
            *s = y as f32;
            self.phase += dt;
            self.phase -= self.phase.floor();
        }
    }
}

/// scsynth's `Formant(fundfreq, formfreq, bwfreq)`: at every cycle of the
/// fundamental, a burst of a sine at `formfreq` under a raised-cosine window
/// `bwfreq` Hz wide (never longer than the fundamental's period) -- one
/// formant, the spectrum's peak at `formfreq` whatever the fundamental.
///
/// Each burst starts where the fundamental's cycle does, within the sample:
/// the carrier's and the window's phases restart from the part of the sample
/// past the crossing. scsynth's restart multiplies two 32-bit phases and
/// overflows, so its carrier starts each burst at a phase that is noise; and
/// it restarts the window by `bwfreq` where it advances it by the larger of
/// the two. Here both restart where they should. The frequencies are read
/// once a block, as scsynth reads them.
pub struct Formant {
    fund: f64,
    carrier: f64,
    window: f64,
}

impl Formant {
    pub fn new() -> Self {
        Self {
            fund: 0.0,
            carrier: 0.0,
            window: 0.0,
        }
    }
}

impl Default for Formant {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Formant {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        let f1 = inputs[0][0] as f64 / sr;
        let f2 = inputs[1][0] as f64 / sr;
        let f3 = (inputs[2][0] as f64 / sr).max(f1);
        for s in output.iter_mut() {
            *s = if self.window < 1.0 {
                let y = (1.0 - (TAU * self.window).cos()) * (TAU * self.carrier).sin();
                self.window += f3;
                y as f32
            } else {
                0.0
            };
            self.fund += f1;
            self.carrier += f2;
            self.carrier -= self.carrier.floor();
            if self.fund > 1.0 {
                self.fund -= 1.0;
                // The time since the crossing, in samples, is fund / f1.
                let since = self.fund / f1;
                self.carrier = since * f2;
                self.carrier -= self.carrier.floor();
                self.window = since * f3;
            }
        }
    }
}
