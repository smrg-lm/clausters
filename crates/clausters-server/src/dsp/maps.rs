//! Maps and tests of a value, scsynth's from its `LFUGens.cpp`: `LinExp`,
//! `InRange`, `InRect`, `ModDif`, `Unwrap`, and the loudness compensations
//! `AmpComp` and `AmpCompA`.
//!
//! Every input is read per sample. scsynth reads several of them once a
//! block (`InRange`'s bounds, `InRect`'s rectangle), which gives the same
//! output for a control-rate input and keeps an audio-rate one's timing.

use clausters_core::warp::{Clip, Map, MapOp};

use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// `LinExp(in, srclo, srchi, dstlo, dsthi)`: `in` mapped linearly from the
/// source range to an exponential destination, `dstlo * (dsthi/dstlo)^t`,
/// unclipped -- scsynth's `LinExp` extrapolates as this does. The map is
/// `clausters_core::warp`'s `linexp`, the one `RangeMapUGen` and a client's
/// value functions compute, with its rule for an end at zero.
pub struct LinExp;

impl UGen for LinExp {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let map = |i: usize| {
            let b = |k: usize| at(inputs[k], i);
            Map::new(MapOp::Linexp, b(1), b(2), b(3), b(4), 0.0, Clip::None)
        };
        if inputs[1..5].iter().all(|b| b.len() == 1) {
            let m = map(0);
            for (i, s) in output.iter_mut().enumerate() {
                *s = m.at(at(inputs[0], i));
            }
        } else {
            for (i, s) in output.iter_mut().enumerate() {
                *s = map(i).at(at(inputs[0], i));
            }
        }
    }
}

/// `InRange(in, lo, hi)`: 1 where `lo <= in <= hi`, else 0.
pub struct InRange;

impl UGen for InRange {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            *s = (x >= at(inputs[1], i) && x <= at(inputs[2], i)) as u8 as f32;
        }
    }
}

/// `InRect(x, y, left, top, right, bottom)`: 1 where the point `(x, y)` lies
/// in the rectangle, its edges included, else 0.
pub struct InRect;

impl UGen for InRect {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, s) in output.iter_mut().enumerate() {
            let (x, y) = (at(inputs[0], i), at(inputs[1], i));
            let inside = x >= at(inputs[2], i)
                && x <= at(inputs[4], i)
                && y >= at(inputs[3], i)
                && y <= at(inputs[5], i);
            *s = inside as u8 as f32;
        }
    }
}

/// `ModDif(x, y, mod)`: the distance from `x` to `y` on a circle of
/// circumference `mod` -- the shorter way round, in `[0, mod/2]`. A
/// control-rate `y` or `mod` is a line across the block, as scsynth's is.
#[derive(Default)]
pub struct ModDif {
    dif: Ramp,
    modulus: Ramp,
}

impl UGen for ModDif {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let n = output.len();
        // One sample a block reads the value itself: a control-rate
        // `ModDif` has nothing to move across.
        let line = |r: &mut Ramp, x: &[f32]| {
            if x.len() == 1 && n > 1 {
                Some(r.toward(x[0] as f64, n))
            } else {
                None
            }
        };
        let dif = line(&mut self.dif, inputs[1]);
        let modulus = line(&mut self.modulus, inputs[2]);
        for (i, s) in output.iter_mut().enumerate() {
            let value = |l: Option<(f64, f64)>, x: &[f32]| match l {
                Some((start, step)) => (start + step * i as f64) as f32,
                None => at(x, i),
            };
            let (y, m) = (value(dif, inputs[1]), value(modulus, inputs[2]));
            let d = (at(inputs[0], i) - y).abs() % m;
            let half = m * 0.5;
            *s = half - (d - half).abs();
        }
    }
}

/// `Unwrap(in, lo, hi)`: undoes a wrap. Each time `in` jumps by more than half
/// the range `hi - lo`, the output adds or takes away a whole range, so a
/// phase that wraps comes out as one that keeps going. The range is read at
/// the start, as scsynth's is, and the output starts inside it.
///
/// scsynth's initial offset puts a start outside the range further outside
/// (`floor((lo - in) / range)` where it means `-floor((in - lo) / range)`), and
/// its first sample compares against an uninitialized previous input; here
/// the first input is brought into the range and the comparison starts from
/// it.
#[derive(Default)]
pub struct Unwrap {
    range: f32,
    offset: f32,
    prev: f32,
    started: bool,
}

impl UGen for Unwrap {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.started {
            self.started = true;
            let x = at(inputs[0], 0);
            let (a, b) = (at(inputs[1], 0), at(inputs[2], 0));
            let (lo, hi) = (a.min(b), a.max(b));
            self.range = hi - lo;
            self.offset = if self.range > 0.0 && (x < lo || x >= hi) {
                -((x - lo) / self.range).floor() * self.range
            } else {
                0.0
            };
            self.prev = x;
        }
        let half = self.range * 0.5;
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            if (x - self.prev).abs() > half {
                if x < self.prev {
                    self.offset += self.range;
                } else {
                    self.offset -= self.range;
                }
            }
            *s = x + self.offset;
            self.prev = x;
        }
    }
}

/// `AmpComp(freq, root, exp)`: `(root / freq)^exp`, the gain that keeps a
/// higher partial from sounding louder than a lower one of the same
/// amplitude -- an exponent of a third is the usual approximation of equal
/// loudness. Sign-preserving, as scsynth's is.
pub struct AmpComp;

impl UGen for AmpComp {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[1], i) / at(inputs[0], i);
            let e = at(inputs[2], i);
            *s = if x >= 0.0 { x.powf(e) } else { -(-x).powf(e) };
        }
    }
}

/// The squared A-weighting's constant, normalized to 0 dB at 1 kHz, and its
/// poles' frequencies squared (IEC 61672's 20.6, 107.7, 737.9 and 12194 Hz,
/// as scsynth writes them).
const A_K: f64 = 3.5041384e16;
const A_C1: f64 = 20.598997 * 20.598997;
const A_C2: f64 = 107.65265 * 107.65265;
const A_C3: f64 = 737.86223 * 737.86223;
const A_C4: f64 = 12194.217 * 12194.217;
/// `1 - A` at the weighting's peak (+1.27 dB near 2.5 kHz): the lowest level
/// [`a_level`] gives.
const A_MIN_LEVEL: f64 = -0.1575371167435;

/// One minus the A-weighting's gain at `freq`.
#[inline]
fn a_level(freq: f64) -> f64 {
    let r = freq * freq;
    let n1 = A_C1 + r;
    let n4 = A_C4 + r;
    let w2 = A_K * r * r * r * r / (n1 * n1 * (A_C2 + r) * (A_C3 + r) * n4 * n4);
    1.0 - w2.sqrt()
}

/// `AmpCompA(freq, root, min_amp, root_amp)`: a loudness compensation that
/// follows the A-weighting curve, so it holds better than [`AmpComp`] at the
/// ends of the range. The gain is `1 - A(freq)` mapped linearly so that
/// `root` gives `root_amp` and the weighting's peak gives `min_amp`. The
/// three bounds are read every block, where scsynth reads them once.
pub struct AmpCompA;

impl UGen for AmpCompA {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let root_level = a_level(at(inputs[1], 0) as f64);
        let min_amp = at(inputs[2], 0) as f64;
        let scale = (at(inputs[3], 0) as f64 - min_amp) / (root_level - A_MIN_LEVEL);
        let offset = min_amp - scale * A_MIN_LEVEL;
        for (i, s) in output.iter_mut().enumerate() {
            *s = (a_level(at(inputs[0], i) as f64) * scale + offset) as f32;
        }
    }
}

/// `Hasher(in)`: the input's bits through scsynth's integer hash, as a value
/// in `[-1, 1)` -- noise that is a function of its input, the same value for
/// the same input every time.
pub struct Hasher;

impl UGen for Hasher {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, s) in output.iter_mut().enumerate() {
            let h = clausters_core::rng::sc_hash(at(inputs[0], i).to_bits() as i32) as u32;
            *s = f32::from_bits(0x4000_0000 | (h >> 9)) - 3.0;
        }
    }
}

/// `MantissaMask(in, bits)`: the input with all but the top `bits` (0 to 23)
/// of its mantissa cleared -- a reduction of precision that keeps the
/// exponent, so it is the same relative coarseness at every level.
pub struct MantissaMask;

impl UGen for MantissaMask {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let bits = (at(inputs[1], 0) as i32).clamp(0, 23);
        let mask = u32::MAX << (23 - bits);
        for (i, s) in output.iter_mut().enumerate() {
            *s = f32::from_bits(at(inputs[0], i).to_bits() & mask);
        }
    }
}
