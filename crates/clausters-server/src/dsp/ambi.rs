//! Panning beyond a pair, scsynth's from its `PanUGens.cpp`: a source on a
//! square of four channels (`Pan4`), and first-order Ambisonics -- a source
//! encoded into B-format in three dimensions (`PanB`) or two (`PanB2`), a
//! pair of opposite sources into two (`BiPanB2`), and a B-format field
//! decoded onto a ring of speakers (`DecodeB2`).
//!
//! Each row is one output channel, its index last on the wire, as the pan
//! family's are ([`super::pan`]); the builder makes one row a channel. Every
//! gain is the trigonometry itself ([`sin_cos_pi`]), where scsynth reads a
//! sine table at a truncated index, a step of up to `7.7e-4` in a gain. A
//! control-rate parameter moves the gain in a line across the block, as
//! scsynth's does; an audio-rate one is read every sample, where scsynth's
//! reads its first.
//!
//! The angles are scsynth's units, not its help's radians: an azimuth of
//! `+/-1` is half a turn either way, and `PanB`'s elevation of `+/-1` a
//! quarter turn up or down.

use std::f64::consts::FRAC_1_SQRT_2;

use crate::dsp::pan::{quarter_sin, sin_cos_pi};
use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// Which encoder a [`BFormat`] row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BKind {
    /// `PanB(in, azimuth, elevation, gain)`: W, X, Y and Z.
    PanB,
    /// `PanB2(in, azimuth, gain)`: W, X and Y.
    PanB2,
    /// `BiPanB2(inA, inB, azimuth, gain)`: `inA` at the azimuth and `inB`
    /// opposite it, W, X and Y.
    BiPanB2,
}

/// First-order B-format encoding, Furse-Malham's weights: W the source at
/// `1/sqrt(2)`, X `cos(az) cos(el)`, Y `-sin(az) cos(el)`, Z `sin(el)` --
/// the minus scsynth's, an azimuth growing clockwise.
pub struct BFormat {
    kind: BKind,
    gain: Ramp,
}

impl BFormat {
    pub fn new(kind: BKind) -> Self {
        Self {
            kind,
            gain: Ramp::default(),
        }
    }

    /// Wire inputs before the azimuth.
    fn sources(&self) -> usize {
        if self.kind == BKind::BiPanB2 { 2 } else { 1 }
    }

    /// The gain of channel `chan` (0 W, 1 X, 2 Y, 3 Z) at a direction.
    #[inline]
    fn weight(chan: usize, azimuth: f64, elevation: f64) -> f64 {
        let (sa, ca) = sin_cos_pi(azimuth);
        let (se, ce) = sin_cos_pi(elevation * 0.5);
        match chan {
            0 => FRAC_1_SQRT_2,
            1 => ca * ce,
            2 => -sa * ce,
            _ => se,
        }
    }
}

impl UGen for BFormat {
    fn reset(&mut self) {
        self.gain = Ramp::default();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let base = self.sources();
        let azimuth = inputs[base];
        let (elevation, level, chan) = match self.kind {
            BKind::PanB => (inputs[base + 1], inputs[base + 2], inputs[base + 3]),
            _ => (&[0.0f32][..], inputs[base + 1], inputs[base + 2]),
        };
        let chan = (chan[0].max(0.0) as usize).min(3);
        let pair = self.kind == BKind::BiPanB2;
        // The source a channel weighs: a pair adds into W and subtracts into
        // X and Y, the second source half a turn round.
        let source = |i: usize| -> f64 {
            if !pair {
                at(inputs[0], i) as f64
            } else if chan == 0 {
                at(inputs[0], i) as f64 + at(inputs[1], i) as f64
            } else {
                at(inputs[0], i) as f64 - at(inputs[1], i) as f64
            }
        };
        let n = output.len();
        if azimuth.len() == 1 && elevation.len() == 1 && level.len() == 1 {
            let g = Self::weight(chan, azimuth[0] as f64, elevation[0] as f64) * level[0] as f64;
            let (g0, dg) = self.gain.toward(g, n);
            for (i, s) in output.iter_mut().enumerate() {
                *s = (source(i) * (g0 + dg * i as f64)) as f32;
            }
            return;
        }
        let mut last = 0.0;
        for (i, s) in output.iter_mut().enumerate() {
            last = Self::weight(chan, at(azimuth, i) as f64, at(elevation, i) as f64)
                * at(level, i) as f64;
            *s = (source(i) * last) as f32;
        }
        self.gain.set(last);
    }
}

/// `DecodeB2(w, x, y, orientation, numchans, chan)`: speaker `chan` of a
/// ring of `numchans`, `W / sqrt(2) + (X cos(t) + Y sin(t)) / 2` at its
/// angle `t = 2 pi (orientation - chan) / numchans` -- a cardioid towards
/// it, the speakers going round clockwise. `orientation` 0 puts speaker 0
/// in front, 0.5 the front between two. Both are read at the start.
///
/// scsynth turns each speaker's angle from the last by a rotation in
/// `f32`, which gathers rounding round the ring; here each is computed.
#[derive(Default)]
pub struct DecodeB2 {
    gains: Option<[f64; 3]>,
}

impl UGen for DecodeB2 {
    fn reset(&mut self) {
        self.gains = None;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let [w, x, y] = *self.gains.get_or_insert_with(|| {
            let n = (inputs[4][0] as f64).max(1.0);
            let chan = inputs[5][0].max(0.0) as f64;
            let (s, c) = sin_cos_pi(2.0 * (inputs[3][0] as f64 - chan) / n);
            [FRAC_1_SQRT_2, 0.5 * c, 0.5 * s]
        });
        for (i, s) in output.iter_mut().enumerate() {
            *s = (at(inputs[0], i) as f64 * w
                + at(inputs[1], i) as f64 * x
                + at(inputs[2], i) as f64 * y) as f32;
        }
    }
}

/// `Pan4(in, xpos, ypos, level, chan)`: a source on a square of four
/// channels (0 left front, 1 right front, 2 left back, 3 right back), equal
/// power across and front to back: `sin` of a quarter turn by the position
/// on each axis. A position off the square is carried to its edge along
/// the line from the centre.
#[derive(Default)]
pub struct Pan4 {
    gain: Ramp,
}

impl Pan4 {
    /// The gain of `chan` at a position.
    fn weight(chan: usize, x: f64, y: f64) -> f64 {
        let (x, y) = to_square(x, y);
        let t = |p: f64| (p * 0.5 + 0.5).clamp(0.0, 1.0);
        let (left, right) = (quarter_sin(1.0 - t(x)), quarter_sin(t(x)));
        let (back, front) = (quarter_sin(1.0 - t(y)), quarter_sin(t(y)));
        match chan {
            0 => left * front,
            1 => right * front,
            2 => left * back,
            _ => right * back,
        }
    }
}

/// A position outside the square `[-1, 1]^2` carried to its edge along the
/// line from the centre, as scsynth's `Pan4` does.
fn to_square(x: f64, y: f64) -> (f64, f64) {
    if (-1.0..=1.0).contains(&x) && (-1.0..=1.0).contains(&y) {
        return (x, y);
    }
    if y > x.abs() {
        (x / y, 1.0)
    } else if y < -x.abs() {
        (-x / y, -1.0)
    } else if y.abs() < x {
        (1.0, y / x)
    } else {
        (-1.0, -y / x)
    }
}

impl UGen for Pan4 {
    fn reset(&mut self) {
        self.gain = Ramp::default();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (sig, x, y, level) = (inputs[0], inputs[1], inputs[2], inputs[3]);
        let chan = (inputs[4][0].max(0.0) as usize).min(3);
        let n = output.len();
        if x.len() == 1 && y.len() == 1 && level.len() == 1 {
            let g = Self::weight(chan, x[0] as f64, y[0] as f64) * level[0] as f64;
            let (g0, dg) = self.gain.toward(g, n);
            for (i, s) in output.iter_mut().enumerate() {
                *s = (at(sig, i) as f64 * (g0 + dg * i as f64)) as f32;
            }
            return;
        }
        let mut last = 0.0;
        for (i, s) in output.iter_mut().enumerate() {
            last = Self::weight(chan, at(x, i) as f64, at(y, i) as f64) * at(level, i) as f64;
            *s = (at(sig, i) as f64 * last) as f32;
        }
        self.gain.set(last);
    }
}
