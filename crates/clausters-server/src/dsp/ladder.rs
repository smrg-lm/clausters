//! The Moog ladder, scsynth's `MoogFF` from its `FilterUGens.cpp`: Federico
//! Fontana's zero-delay-feedback form, four bilinear one-poles with the
//! feedback solved exactly rather than delayed by a sample, so the
//! resonance and the cutoff stay where they are asked to be.

use std::f64::consts::PI;

use crate::dsp::sections::flush;
use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// `MoogFF(in, freq, gain, reset)`: a four-pole lowpass, `-24 dB` an octave,
/// with resonance `gain` (0 to 4, self-oscillating at 4); a positive
/// `reset` clears its state. A control-rate `gain` is a line across the
/// block, as scsynth's is.
#[derive(Default)]
pub struct MoogFf {
    s: [f64; 4],
    freq: Option<f32>,
    b0: f64,
    a1: f64,
    k: Ramp,
}

impl UGen for MoogFf {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if at(inputs[3], 0) > 0.0 {
            self.s = [0.0; 4];
        }
        let freq = at(inputs[1], 0);
        if self.freq != Some(freq) {
            self.freq = Some(freq);
            let sr = ctx.sample_rate as f64;
            let t = 1.0 / sr;
            let wc = (2.0 * (t * PI * freq as f64).tan() * sr).max(0.0);
            let twc = t * wc;
            self.b0 = twc / (twc + 2.0);
            self.a1 = (twc - 2.0) / (twc + 2.0);
        }
        let (b0, a1) = (self.b0, self.a1);
        let b04 = b0 * b0 * b0 * b0;
        let k_in = inputs[2];
        let (mut k, step) = if k_in.len() == 1 {
            self.k.toward(k_in[0].clamp(0.0, 4.0) as f64, output.len())
        } else {
            (0.0, 0.0)
        };
        let [mut s1, mut s2, mut s3, mut s4] = self.s;
        for (i, out) in output.iter_mut().enumerate() {
            let kk = if k_in.len() == 1 {
                k
            } else {
                k_in[i].clamp(0.0, 4.0) as f64
            };
            let o = s4 + b0 * (s3 + b0 * (s2 + b0 * s1));
            let x = at(inputs[0], i) as f64;
            let y = (b04 * x + o) / (1.0 + b04 * kk);
            *out = y as f32;
            let u = x - kk * y;
            let mut past = u;
            let mut future = b0 * past + s1;
            s1 = b0 * past - a1 * future;
            past = future;
            future = b0 * past + s2;
            s2 = b0 * past - a1 * future;
            past = future;
            future = b0 * past + s3;
            s3 = b0 * past - a1 * future;
            s4 = b0 * future - a1 * y;
            k += step;
        }
        self.s = [flush(s1), flush(s2), flush(s3), flush(s4)];
    }
}
