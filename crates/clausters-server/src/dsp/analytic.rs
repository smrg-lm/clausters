//! The analytic signal, scsynth's from its `FilterUGens.cpp`: `Hilbert`,
//! the input as two signals a quarter cycle apart, and `FreqShift`, which
//! moves every frequency by the same number of hertz with it.
//!
//! The pair is two chains of six first-order allpasses whose phase
//! responses differ by 90 degrees across the audio band (to within a
//! degree from about 15 Hz to 20 kHz at 48 kHz), scsynth's design, its
//! poles placed at the sample rate the synth runs at.

use std::f64::consts::{PI, TAU};

use crate::dsp::sections::flush;
use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// The poles of the two chains, scsynth's, in hertz over 15 pi.
const POLES: [f64; 12] = [
    0.3609, 2.7412, 11.1573, 44.7581, 179.6242, 798.4578, 1.2524, 5.5671, 22.3423, 89.6271,
    364.7914, 2770.1114,
];

/// The two chains: their coefficients and their states.
struct Pair {
    coefs: [f64; 12],
    y1: [f64; 12],
}

impl Pair {
    fn new(sample_rate: f32) -> Self {
        let g = 15.0 * PI / sample_rate as f64;
        let mut coefs = [0.0; 12];
        for (c, p) in coefs.iter_mut().zip(POLES) {
            // scsynth's poles are single-precision constants.
            let gamma = g * p as f32 as f64;
            *c = (gamma - 1.0) / (gamma + 1.0);
        }
        Self {
            coefs,
            y1: [0.0; 12],
        }
    }

    /// The in-phase and quadrature outputs for one input sample.
    #[inline]
    fn step(&mut self, x: f64) -> (f64, f64) {
        let mut chain = |from: usize| {
            let mut v = x;
            for k in from..from + 6 {
                let y0 = v - self.coefs[k] * self.y1[k];
                v = self.coefs[k] * y0 + self.y1[k];
                self.y1[k] = y0;
            }
            v
        };
        let cos = chain(0);
        let sin = chain(6);
        (cos, sin)
    }

    fn flush(&mut self) {
        for y in self.y1.iter_mut() {
            *y = flush(*y);
        }
    }
}

/// `Hilbert(in)`: the input as two signals 90 degrees apart, channel 0 the
/// real part and 1 the imaginary -- the analytic signal, whose magnitude is
/// the input's envelope. Input 1 chooses the channel.
pub struct Hilbert {
    pair: Option<Pair>,
}

impl Hilbert {
    pub fn new() -> Self {
        Self { pair: None }
    }
}

impl Default for Hilbert {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Hilbert {
    fn reset(&mut self) {
        self.pair = None;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let pair = self.pair.get_or_insert_with(|| Pair::new(ctx.sample_rate));
        let imaginary = at(inputs[1], 0) >= 0.5;
        for (i, s) in output.iter_mut().enumerate() {
            let (re, im) = pair.step(at(inputs[0], i) as f64);
            *s = if imaginary { im } else { re } as f32;
        }
        pair.flush();
    }
}

/// `FreqShift(in, freq, phase)`: every frequency of the input moved by
/// `freq` hertz (down for a negative one) -- single-sideband modulation of
/// the analytic signal by a quadrature oscillator. `phase` offsets the
/// oscillator, in cycles (scsynth's in radians); a control-rate `phase` is a
/// line across the block.
///
/// The oscillator is computed where scsynth reads a table. scsynth's
/// constructor runs a sample of the oscillator, so its oscillator is a
/// sample ahead; and it scales an initial control-rate `phase` by a factor
/// it has not yet set.
pub struct FreqShift {
    pair: Option<Pair>,
    phase: f64,
    offset: Ramp,
}

impl FreqShift {
    pub fn new() -> Self {
        Self {
            pair: None,
            phase: 0.0,
            offset: Ramp::default(),
        }
    }
}

impl Default for FreqShift {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for FreqShift {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let dt = 1.0 / ctx.sample_rate as f64;
        let pair = self.pair.get_or_insert_with(|| Pair::new(ctx.sample_rate));
        let phase_in = inputs[2];
        let (mut off, step) = if phase_in.len() == 1 {
            self.offset.toward(phase_in[0] as f64, output.len())
        } else {
            (0.0, 0.0)
        };
        for (i, s) in output.iter_mut().enumerate() {
            let (re, im) = pair.step(at(inputs[0], i) as f64);
            let o = if phase_in.len() == 1 {
                off
            } else {
                phase_in[i] as f64
            };
            let theta = TAU * (self.phase + o);
            *s = (re * theta.cos() + im * theta.sin()) as f32;
            off += step;
            self.phase += at(inputs[1], i) as f64 * dt;
            self.phase -= self.phase.floor();
        }
        pair.flush();
    }
}
