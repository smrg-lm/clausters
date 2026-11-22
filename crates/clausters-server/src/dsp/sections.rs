//! The small sections of scsynth's `FilterUGens.cpp`: the fixed FIRs
//! (`Delay1`, `Delay2`, `LPZ1`, `HPZ1`, `LPZ2`, `HPZ2`, `BPZ2`, `BRZ2`,
//! `Slope`, `Flip`), the two-zero `TwoZero`, the sections whose coefficients
//! are the inputs (`FOS`, `SOS`), and the two shapers of a signal's movement,
//! `Ramp` and `Slew`.
//!
//! Each is scsynth's difference equation, in `f64` where scsynth's state is
//! `double`. The sections with a pole and a named parameter (`TwoPole`,
//! `Ringz`, the `B*` family) are in [`crate::dsp::filter`], realized by the
//! state-variable filter; `FOS` and `SOS` stay direct, because their inputs
//! *are* the direct form's coefficients, and a user who writes them means that
//! difference equation, stable or not.
//!
//! A FIR's history starts at the first input sample, which is where scsynth's
//! constructor leaves it.

use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// Which fixed FIR a [`Fir`] is: `y[n]` from `x[n]`, `x[n-1]`, `x[n-2]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirKind {
    /// `LPZ1`: `0.5 (x0 + x1)`, a zero at Nyquist.
    Lpz1,
    /// `HPZ1`: `0.5 (x0 - x1)`, a zero at DC.
    Hpz1,
    /// `LPZ2`: `0.25 (x0 + 2 x1 + x2)`.
    Lpz2,
    /// `HPZ2`: `0.25 (x0 - 2 x1 + x2)`.
    Hpz2,
    /// `BPZ2`: `0.5 (x0 - x2)`, zeros at DC and Nyquist.
    Bpz2,
    /// `BRZ2`: `0.5 (x0 + x2)`, zeros at a quarter of the rate.
    Brz2,
    /// `Slope`: `rate * (x0 - x1)`, the derivative per second.
    Slope,
}

/// The fixed FIRs: one input, the signal.
pub struct Fir {
    kind: FirKind,
    x1: f64,
    x2: f64,
    primed: bool,
}

impl Fir {
    pub fn new(kind: FirKind) -> Self {
        Self {
            kind,
            x1: 0.0,
            x2: 0.0,
            primed: false,
        }
    }
}

impl UGen for Fir {
    fn reset(&mut self) {
        self.primed = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sig = inputs[0];
        if !self.primed {
            self.primed = true;
            self.x1 = sig[0] as f64;
            self.x2 = self.x1;
        }
        let sr = ctx.sample_rate as f64;
        for (i, s) in output.iter_mut().enumerate() {
            let x0 = at(sig, i) as f64;
            let (x1, x2) = (self.x1, self.x2);
            let y = match self.kind {
                FirKind::Lpz1 => 0.5 * (x0 + x1),
                FirKind::Hpz1 => 0.5 * (x0 - x1),
                FirKind::Lpz2 => (x0 + 2.0 * x1 + x2) * 0.25,
                FirKind::Hpz2 => (x0 - 2.0 * x1 + x2) * 0.25,
                FirKind::Bpz2 => (x0 - x2) * 0.5,
                FirKind::Brz2 => (x0 + x2) * 0.5,
                FirKind::Slope => sr * (x0 - x1),
            };
            self.x2 = x1;
            self.x1 = x0;
            *s = y as f32;
        }
    }
}

/// `Delay1(in, x1)` and `Delay2(in, x1, x2)`: the input one or two samples
/// late. The trailing inputs are the history the line starts with -- what
/// the first one or two samples are -- read once.
pub struct SampleDelay {
    two: bool,
    x1: f32,
    x2: f32,
    primed: bool,
}

impl SampleDelay {
    pub fn new(two: bool) -> Self {
        Self {
            two,
            x1: 0.0,
            x2: 0.0,
            primed: false,
        }
    }
}

impl UGen for SampleDelay {
    fn reset(&mut self) {
        self.primed = false;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.primed {
            self.primed = true;
            self.x1 = inputs.get(1).map_or(0.0, |x| x[0]);
            self.x2 = inputs.get(2).map_or(0.0, |x| x[0]);
        }
        for (i, s) in output.iter_mut().enumerate() {
            let x0 = at(inputs[0], i);
            if self.two {
                *s = self.x2;
                self.x2 = self.x1;
            } else {
                *s = self.x1;
            }
            self.x1 = x0;
        }
    }
}

/// `Flip(in)`: every other sample negated, the first among them -- the
/// spectrum mirrored about a quarter of the rate. Which samples is counted from the synth's start, so
/// a split block keeps the alternation.
#[derive(Default)]
pub struct Flip {
    odd: bool,
}

impl UGen for Flip {
    fn reset(&mut self) {
        self.odd = false;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            *s = if self.odd { x } else { -x };
            self.odd = !self.odd;
        }
    }
}

/// `TwoZero(in, freq, radius)`: two zeros at `freq` with `radius`,
/// `y = x0 - 2 r cos(w) x1 + r^2 x2`. A changing `freq` or `radius` moves
/// the coefficients across the block, as scsynth's does.
#[derive(Default)]
pub struct TwoZero {
    x1: f64,
    x2: f64,
    b1: Ramp,
    b2: Ramp,
}

impl UGen for TwoZero {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let n = output.len();
        let (freq, reson) = (inputs[1][0] as f64, inputs[2][0] as f64);
        let w = freq * std::f64::consts::TAU / ctx.sample_rate as f64;
        let (mut b1, d1) = self.b1.toward(-2.0 * reson * w.cos(), n);
        let (mut b2, d2) = self.b2.toward(reson * reson, n);
        for (i, s) in output.iter_mut().enumerate() {
            b1 += d1;
            b2 += d2;
            let x0 = at(inputs[0], i) as f64;
            *s = (x0 + b1 * self.x1 + b2 * self.x2) as f32;
            self.x2 = self.x1;
            self.x1 = x0;
        }
    }
}

/// `FOS(in, a0, a1, b1)`: the first-order section with its coefficients as
/// inputs, `w = x + b1 w1`, `y = a0 w + a1 w1`. Control-rate coefficients move
/// across the block; audio-rate ones are read per sample.
pub struct Fos {
    y1: f64,
    coef: [Ramp; 3],
}

impl Default for Fos {
    fn default() -> Self {
        Self {
            y1: 0.0,
            coef: [Ramp::default(); 3],
        }
    }
}

impl UGen for Fos {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let n = output.len();
        let mut c = [(0.0, 0.0); 3];
        for (k, slot) in c.iter_mut().enumerate() {
            *slot = self.coef[k].toward(inputs[1 + k][0] as f64, n);
        }
        let audio = inputs[1..4].iter().any(|x| x.len() > 1);
        for (i, s) in output.iter_mut().enumerate() {
            let [a0, a1, b1] = if audio {
                [
                    at(inputs[1], i) as f64,
                    at(inputs[2], i) as f64,
                    at(inputs[3], i) as f64,
                ]
            } else {
                let v = [c[0].0, c[1].0, c[2].0];
                for slot in c.iter_mut() {
                    slot.0 += slot.1;
                }
                v
            };
            let y0 = at(inputs[0], i) as f64 + b1 * self.y1;
            *s = (a0 * y0 + a1 * self.y1) as f32;
            self.y1 = flush(y0);
        }
    }
}

/// `SOS(in, a0, a1, a2, b1, b2)`: the second-order section with its
/// coefficients as inputs, `w = x + b1 w1 + b2 w2`, `y = a0 w + a1 w1 + a2
/// w2`. Control-rate coefficients move across the block; audio-rate ones are
/// read per sample.
pub struct Sos {
    y1: f64,
    y2: f64,
    coef: [Ramp; 5],
}

impl Default for Sos {
    fn default() -> Self {
        Self {
            y1: 0.0,
            y2: 0.0,
            coef: [Ramp::default(); 5],
        }
    }
}

impl UGen for Sos {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let n = output.len();
        let mut c = [(0.0, 0.0); 5];
        for (k, slot) in c.iter_mut().enumerate() {
            *slot = self.coef[k].toward(inputs[1 + k][0] as f64, n);
        }
        let audio = inputs[1..6].iter().any(|x| x.len() > 1);
        for (i, s) in output.iter_mut().enumerate() {
            let k: [f64; 5] = if audio {
                std::array::from_fn(|j| at(inputs[1 + j], i) as f64)
            } else {
                let v = std::array::from_fn(|j| c[j].0);
                for slot in c.iter_mut() {
                    slot.0 += slot.1;
                }
                v
            };
            let y0 = at(inputs[0], i) as f64 + k[3] * self.y1 + k[4] * self.y2;
            *s = (k[0] * y0 + k[1] * self.y1 + k[2] * self.y2) as f32;
            self.y2 = self.y1;
            self.y1 = flush(y0);
        }
    }
}

/// `Ramp(in, lagTime)`: the input sampled every `lagTime` seconds and joined
/// by straight lines -- a stepped signal smoothed into segments, each
/// arriving at the value the input had when it started. scsynth's: the
/// output starts at the first input, and the first segment starts at once.
pub struct LineRamp {
    level: f64,
    slope: f64,
    counter: i64,
    primed: bool,
}

impl Default for LineRamp {
    fn default() -> Self {
        Self {
            level: 0.0,
            slope: 0.0,
            counter: 1,
            primed: false,
        }
    }
}

impl UGen for LineRamp {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (sig, period) = (inputs[0], inputs[1][0]);
        if !self.primed {
            self.primed = true;
            self.level = sig[0] as f64;
        }
        let sr = ctx.sample_rate as f64;
        let len = output.len();
        for (i, s) in output.iter_mut().enumerate() {
            *s = self.level as f32;
            self.level += self.slope;
            self.counter -= 1;
            if self.counter <= 0 {
                self.counter = ((period as f64 * sr) as i64).max(1);
                // The input the next segment aims at: the sample after this
                // one, as scsynth reads it.
                let next = at(sig, (i + 1).min(len - 1)) as f64;
                self.slope = (next - self.level) / self.counter as f64;
            }
        }
    }
}

/// `Slew(in, up, dn)`: the input followed at no more than `up` units per
/// second rising and `dn` falling. The output starts at the first input.
pub struct Slew {
    level: f64,
    primed: bool,
}

impl Default for Slew {
    fn default() -> Self {
        Self {
            level: 0.0,
            primed: false,
        }
    }
}

impl UGen for Slew {
    fn reset(&mut self) {
        self.primed = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sig = inputs[0];
        if !self.primed {
            self.primed = true;
            self.level = sig[0] as f64;
        }
        let dur = (1.0 / ctx.sample_rate as f64) as f32;
        let up = (inputs[1][0] * dur) as f64;
        let dn = (0.0 - inputs[2][0] * dur) as f64;
        for (i, s) in output.iter_mut().enumerate() {
            let slope = at(sig, i) as f64 - self.level;
            self.level += slope.clamp(dn.min(up), up.max(dn));
            *s = self.level as f32;
        }
    }
}

/// scsynth's `zapgremlins`: a state that has decayed into the denormal range
/// or past any sound is zero.
#[inline]
pub(crate) fn flush(x: f64) -> f64 {
    let a = x.abs();
    if a > 1e-15 && a < 1e15 { x } else { 0.0 }
}

/// The longest `Median` window, scsynth's.
const MEDIAN_MAX: usize = 31;

/// `Median(length, in)`: the median of the last `length` samples (1 to 31,
/// odd for a true median), scsynth's sorted window, each value aged so the
/// oldest leaves as the newest arrives. `length` is read at the start. A
/// spike shorter than half the window is removed and an edge kept, which
/// is what sets it apart from a lowpass.
pub struct Median {
    values: [f32; MEDIAN_MAX],
    ages: [u8; MEDIAN_MAX],
    size: usize,
    started: bool,
}

impl Default for Median {
    fn default() -> Self {
        Self {
            values: [0.0; MEDIAN_MAX],
            ages: [0; MEDIAN_MAX],
            size: 1,
            started: false,
        }
    }
}

impl Median {
    fn insert(&mut self, x: f32) -> f32 {
        let n = self.size;
        let last = (n - 1) as u8;
        let mut pos = 0;
        for i in 0..n {
            if self.ages[i] == last {
                pos = i;
            } else {
                self.ages[i] += 1;
            }
        }
        while pos != 0 && x < self.values[pos - 1] {
            self.values[pos] = self.values[pos - 1];
            self.ages[pos] = self.ages[pos - 1];
            pos -= 1;
        }
        while pos != n - 1 && x > self.values[pos + 1] {
            self.values[pos] = self.values[pos + 1];
            self.ages[pos] = self.ages[pos + 1];
            pos += 1;
        }
        self.values[pos] = x;
        self.ages[pos] = 0;
        self.values[n >> 1]
    }
}

impl UGen for Median {
    fn reset(&mut self) {
        self.started = false;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.started {
            self.started = true;
            self.size = (at(inputs[0], 0) as i64).clamp(1, MEDIAN_MAX as i64) as usize;
            let first = at(inputs[1], 0);
            for i in 0..self.size {
                self.values[i] = first;
                self.ages[i] = i as u8;
            }
        }
        for (i, s) in output.iter_mut().enumerate() {
            *s = self.insert(at(inputs[1], i));
        }
    }
}
