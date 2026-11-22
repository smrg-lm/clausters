//! Noise sources: the stochastic generators, each scsynth's algorithm over
//! scsynth's generator.
//!
//! **Every generator here draws from its synth's generator in the engine's
//! table** ([`crate::dsp::rand`]): the one the synth selects (`RandID`, 0
//! unless told), seeded from the server's seed. So a render is reproducible --
//! the same score and the same seed replay the same samples, which is what
//! lets a noisy patch have a golden file at all -- and two sources in one graph
//! are two runs of one stream, drawn in turn, never the same samples. What
//! each one draws, and how it shapes the draws, is scsynth's `NoiseUGens.cpp`:
//! the same generator, the same conversions, the same order of draws.
//!
//! The one thing that differs is when the first draw happens. A scsynth
//! constructor runs one sample of its calculation before the first block, so
//! the stream a scsynth source plays starts one sample's draws in; here the
//! first sample is the first draw. The streams are the same, a sample apart.
//!
//! The families:
//!
//! - **Spectral shapes** -- `WhiteNoise` (flat), `PinkNoise` (-3 dB/octave),
//!   `BrownNoise` (-6). Each is measured, not asserted by construction.
//! - **Bit and sign sources** -- `GrayNoise` (one random bit flipped per
//!   sample), `ClipNoise` (+/-1 only).
//! - **Held and interpolated** -- `LFNoise0`/`LFNoise1`/`LFNoise2` and
//!   `LFClipNoise`: a new random value every `1/freq` seconds, held, ramped or
//!   curved between. These are modulation sources, deliberately not band
//!   limited, like the `LF*` oscillators.
//! - **Impulsive and chaotic** -- `Dust`/`Dust2` (random impulses at a mean
//!   density) and `Crackle` (a chaotic map from a random start).

use crate::dsp::rand::SynthRand;
use crate::dsp::{ProcessCtx, UGen, at};

/// White noise in `[-1, 1)`: scsynth's `frand2` per sample. No inputs.
pub struct WhiteNoise {
    rand: SynthRand,
}

impl WhiteNoise {
    /// One drawing from a table of its own, seeded from `seed`.
    pub fn with_seed(seed: u64) -> Self {
        Self::new(SynthRand::detached(seed))
    }

    pub fn new(rand: SynthRand) -> Self {
        Self { rand }
    }
}

impl UGen for WhiteNoise {
    fn reset(&mut self) {
        // The generator's stream goes on: a restarted voice draws new numbers.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        self.rand.draw(|g| {
            for s in output.iter_mut() {
                *s = g.frand2();
            }
        });
    }
}

/// Number of dice in the Voss-McCartney sum: sixteen, scsynth's.
const PINK_ROWS: usize = 16;

/// `PinkNoise`: equal energy per octave, -3 dB/octave. No inputs.
///
/// **Voss-McCartney**, scsynth's: sixteen dice, and the die re-rolled at each
/// sample is the one picked by the number of trailing zeros in a random word,
/// so die `k` changes once every `2^k` samples on average and **exactly one**
/// changes per sample -- a fixed cost, every sample, forever, which is what
/// an audio callback wants (Trammell's variant is cheaper on average and
/// unbounded in the worst case). The output is the dice's sum plus one fresh
/// draw, read as the mantissa of a float in `[-1, 1)`. Its *peak* reaches
/// +/-1 only when all seventeen agree, so it is a quiet signal -- around 0.13
/// RMS against white noise's 0.58.
pub struct PinkNoise {
    rand: SynthRand,
    dice: [u32; PINK_ROWS],
    total: u32,
    primed: bool,
}

impl PinkNoise {
    /// One drawing from a table of its own, seeded from `seed`.
    pub fn with_seed(seed: u64) -> Self {
        Self::new(SynthRand::detached(seed))
    }

    pub fn new(rand: SynthRand) -> Self {
        Self {
            rand,
            dice: [0; PINK_ROWS],
            total: 0,
            primed: false,
        }
    }
}

impl UGen for PinkNoise {
    fn reset(&mut self) {
        // The generator's stream goes on: a restarted voice draws new numbers.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        let (dice, total, primed) = (&mut self.dice, &mut self.total, &mut self.primed);
        self.rand.draw(|g| {
            if !*primed {
                *primed = true;
                *total = 0;
                for d in dice.iter_mut() {
                    *d = g.trand() >> 13;
                    *total = total.wrapping_add(*d);
                }
            }
            for s in output.iter_mut() {
                let counter = g.trand();
                let fresh = counter >> 13;
                let k = (counter.trailing_zeros() as usize) & (PINK_ROWS - 1);
                let prev = dice[k];
                dice[k] = fresh;
                *total = total.wrapping_add(fresh.wrapping_sub(prev));
                let extra = g.trand() >> 13;
                *s = f32::from_bits(total.wrapping_add(extra) | 0x4000_0000) - 3.0;
            }
        });
    }
}

/// `BrownNoise`: a random walk, -6 dB/octave. No inputs.
///
/// scsynth's: a step of up to an eighth either way per sample (`frand8`),
/// starting from a random level. The walk **reflects** at +/-1 rather than
/// clamping: clamping would let the signal rest against a rail -- a constant,
/// which is a click on the way in and silence while it lasts; reflecting keeps
/// it moving and keeps the distribution flat.
pub struct BrownNoise {
    rand: SynthRand,
    z: f32,
    primed: bool,
}

impl BrownNoise {
    /// One drawing from a table of its own, seeded from `seed`.
    pub fn with_seed(seed: u64) -> Self {
        Self::new(SynthRand::detached(seed))
    }

    pub fn new(rand: SynthRand) -> Self {
        Self {
            rand,
            z: 0.0,
            primed: false,
        }
    }
}

impl UGen for BrownNoise {
    fn reset(&mut self) {
        // The generator's stream goes on: a restarted voice draws new numbers.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        let (z, primed) = (&mut self.z, &mut self.primed);
        self.rand.draw(|g| {
            if !*primed {
                *primed = true;
                *z = g.frand2();
            }
            for s in output.iter_mut() {
                *z += g.frand8();
                if *z > 1.0 {
                    *z = 2.0 - *z;
                } else if *z < -1.0 {
                    *z = -2.0 - *z;
                }
                *s = *z;
            }
        });
    }
}

/// `GrayNoise`: one randomly chosen bit of a 32-bit word flipped per sample,
/// the word read as the output. No inputs.
///
/// Two things about it are easy to get wrong. Its **spectrum is not flat**: the
/// high bits flip rarely -- one sample in 32 for the top one -- and the low bits
/// carry almost no weight, so the energy leans low, measured at -2.9 dB/octave,
/// near enough pink. And its **distribution** is what the kind is really for:
/// consecutive samples differ by exactly one power of two, so the steps span
/// every order of magnitude (the mean step is some four thousand times the
/// median, against 1.14 for white noise) and it sounds grainy rather than
/// smooth. That bit-level property is exact in the **integer** -- bit 31 is the
/// sign bit, which is what makes the output bipolar -- but it is not recoverable
/// from the output, because `word / 2^31` in `f32` has a 24-bit significand
/// against the word's 31 and rounds by an amount that depends on the magnitude
/// the flip just changed.
pub struct GrayNoise {
    rand: SynthRand,
    word: i32,
}

impl GrayNoise {
    /// One drawing from a table of its own, seeded from `seed`.
    pub fn with_seed(seed: u64) -> Self {
        Self::new(SynthRand::detached(seed))
    }

    pub fn new(rand: SynthRand) -> Self {
        Self { rand, word: 0 }
    }
}

impl UGen for GrayNoise {
    fn reset(&mut self) {
        // The generator's stream goes on: a restarted voice draws new numbers.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        let word = &mut self.word;
        self.rand.draw(|g| {
            for s in output.iter_mut() {
                *word ^= 1i32 << (g.trand() & 31);
                *s = *word as f32 * (1.0 / 2_147_483_648.0);
            }
        });
    }
}

/// `ClipNoise`: -1 or 1, nothing between. No inputs.
///
/// A coin flip per sample (scsynth's `fcoin`). Its spectrum is flat like white
/// noise's, but every sample is at full scale, so it is the loudest noise
/// available at a given peak -- which is the reason to reach for it and the
/// reason to be careful.
pub struct ClipNoise {
    rand: SynthRand,
}

impl ClipNoise {
    /// One drawing from a table of its own, seeded from `seed`.
    pub fn with_seed(seed: u64) -> Self {
        Self::new(SynthRand::detached(seed))
    }

    pub fn new(rand: SynthRand) -> Self {
        Self { rand }
    }
}

impl UGen for ClipNoise {
    fn reset(&mut self) {
        // The generator's stream goes on: a restarted voice draws new numbers.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        self.rand.draw(|g| {
            for s in output.iter_mut() {
                *s = g.fcoin();
            }
        });
    }
}

/// How a [`LfNoise`] gets from one random value to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LfNoiseShape {
    /// `LFNoise0`: hold the value for the whole segment (a step).
    Step,
    /// `LFClipNoise`: hold, but the values are only +/-1.
    Clip,
    /// `LFNoise1`: ramp linearly to the next value.
    Linear,
    /// `LFNoise2`: a quadratic through the midpoints, so the *slope* is
    /// continuous too and there is no corner at a segment boundary.
    Quadratic,
}

/// `LFNoise0`, `LFClipNoise`, `LFNoise1` and `LFNoise2`: a new random value
/// every `1/freq` seconds. Input 0 `freq`. scsynth's, draw for draw.
///
/// Not band limited, and deliberately so -- like the `LF*` oscillators these are
/// modulation shapes. A step is a step, and asking for one at audio rate is
/// asking for its harmonics.
///
/// The segment length is the rate over the frequency, truncated, and it is
/// worked out **only at a segment boundary**, from the frequency at that
/// moment: a modulated `freq` changes how long the *next* segment lasts and
/// never stretches the one already running, which would make the value jump.
/// A segment is at least one sample, two for the quadratic, whose curve needs
/// an interior to bend over.
pub struct LfNoise {
    rand: SynthRand,
    shape: LfNoiseShape,
    /// Samples left in the current segment.
    counter: i32,
    level: f32,
    slope: f32,
    curve: f32,
    /// `Quadratic` only: the value drawn for the *next* segment and the
    /// midpoint the current one aims at.
    next_value: f32,
    next_midpoint: f32,
    primed: bool,
}

impl LfNoise {
    /// One drawing from a table of its own, seeded from `seed`.
    pub fn with_seed(shape: LfNoiseShape, seed: u64) -> Self {
        Self::new(shape, SynthRand::detached(seed))
    }

    pub fn new(shape: LfNoiseShape, rand: SynthRand) -> Self {
        Self {
            rand,
            shape,
            counter: 0,
            level: 0.0,
            slope: 0.0,
            curve: 0.0,
            next_value: 0.0,
            next_midpoint: 0.0,
            primed: false,
        }
    }
}

impl UGen for LfNoise {
    fn reset(&mut self) {
        // The generator's stream goes on: a restarted voice draws new numbers.
        *self = Self::new(self.shape, self.rand.clone());
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        let shape = self.shape;
        let Self {
            rand,
            counter,
            level,
            slope,
            curve,
            next_value,
            next_midpoint,
            primed,
            ..
        } = self;
        rand.draw(|g| {
            if !*primed {
                *primed = true;
                // scsynth's constructors: the ramp starts from a random level,
                // the curve aims at the midpoint of a first random value.
                match shape {
                    LfNoiseShape::Linear => *level = g.frand2(),
                    LfNoiseShape::Quadratic => {
                        *next_value = g.frand2();
                        *next_midpoint = *next_value * 0.5;
                    }
                    _ => {}
                }
            }
            for (i, s) in output.iter_mut().enumerate() {
                if *counter <= 0 {
                    let freq = at(inputs[0], i).max(0.001);
                    let n = (sr / freq as f64) as i32;
                    match shape {
                        LfNoiseShape::Step => {
                            *counter = n.max(1);
                            *level = g.frand2();
                        }
                        LfNoiseShape::Clip => {
                            *counter = n.max(1);
                            *level = g.fcoin();
                        }
                        LfNoiseShape::Linear => {
                            *counter = n.max(1);
                            let next = g.frand2();
                            *slope = (next - *level) / *counter as f32;
                        }
                        LfNoiseShape::Quadratic => {
                            let value = *next_value;
                            *next_value = g.frand2();
                            *level = *next_midpoint;
                            *next_midpoint = (*next_value + value) * 0.5;
                            *counter = n.max(2);
                            let len = *counter as f32;
                            *curve =
                                2.0 * (*next_midpoint - *level - len * *slope) / (len * len + len);
                        }
                    }
                }
                *counter -= 1;
                *s = *level;
                match shape {
                    LfNoiseShape::Step | LfNoiseShape::Clip => {}
                    LfNoiseShape::Linear => *level += *slope,
                    LfNoiseShape::Quadratic => {
                        *slope += *curve;
                        *level += *slope;
                    }
                }
            }
        });
    }
}

/// Whether a [`Dust`] fires one-sided or two-sided impulses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DustMode {
    /// `Dust`: impulses in [0, 1).
    Unipolar,
    /// `Dust2`: impulses in [-1, 1).
    Bipolar,
}

/// `Dust` and `Dust2`: random impulses at a mean `density` per second. Input 0
/// `density`.
///
/// Each sample is an independent trial, so the intervals are exponentially
/// distributed and the density is a **mean**, not a rate: a `Dust(10)` will not
/// give you ten evenly spaced impulses, it gives you ten on average with
/// clusters and gaps. That is the difference from `Impulse`, and the reason to
/// use one rather than the other.
///
/// The amplitudes are random too -- the trial's own value, rescaled -- which is
/// scsynth's behaviour and worth knowing before using `Dust` as a clock. The
/// density is read once a block, as scsynth's is.
pub struct Dust {
    rand: SynthRand,
    mode: DustMode,
}

impl Dust {
    /// One drawing from a table of its own, seeded from `seed`.
    pub fn with_seed(mode: DustMode, seed: u64) -> Self {
        Self::new(mode, SynthRand::detached(seed))
    }

    pub fn new(mode: DustMode, rand: SynthRand) -> Self {
        Self { rand, mode }
    }
}

impl UGen for Dust {
    fn reset(&mut self) {
        // The generator's stream goes on: a restarted voice draws new numbers.
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let thresh = inputs[0][0] * (1.0 / ctx.sample_rate as f64) as f32;
        let mode = self.mode;
        let scale = if thresh > 0.0 {
            match mode {
                DustMode::Unipolar => 1.0 / thresh,
                DustMode::Bipolar => 2.0 / thresh,
            }
        } else {
            0.0
        };
        self.rand.draw(|g| {
            for s in output.iter_mut() {
                let z = g.frand();
                *s = if z < thresh {
                    match mode {
                        DustMode::Unipolar => z * scale,
                        DustMode::Bipolar => z * scale - 1.0,
                    }
                } else {
                    0.0
                };
            }
        });
    }
}

/// `Crackle(chaos)`: the chaotic map `y[n] = |chaos*y[n-1] - y[n-2] - 0.05|`.
/// Input 0 `chaos`.
///
/// **Not a random process.** Only its starting point is drawn, as scsynth's is;
/// from there it is deterministic, and the same `chaos` always gives the same
/// signal from the same start. It runs in `f32`, as scsynth's does: a chaotic
/// orbit computed in another width parts from scsynth's within a few hundred
/// samples. What makes it
/// noise-like is that the orbit does not close -- measured here, no period up to
/// 512 samples -- and that `chaos` changes the signal drastically and **not
/// monotonically**: the spread runs 0.56, 0.20, 0.08, 0.05, 0.19, 0.05, 0.06
/// across chaos 0.3 to 1.9. It is a map, not a level control, and the way to
/// use it is by ear. Its output is
/// **unipolar** (the absolute value is part of the map), so it carries DC;
/// subtract its mean or pass it through `LeakDC` before summing it into a bus.
pub struct Crackle {
    rand: SynthRand,
    y1: f32,
    y2: f32,
    primed: bool,
}

impl Crackle {
    /// One drawing from a table of its own, seeded from `seed`.
    pub fn with_seed(seed: u64) -> Self {
        Self::new(SynthRand::detached(seed))
    }

    pub fn new(rand: SynthRand) -> Self {
        Self {
            rand,
            y1: 0.0,
            y2: 0.0,
            primed: false,
        }
    }
}

impl UGen for Crackle {
    fn reset(&mut self) {
        self.primed = false;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.primed {
            self.primed = true;
            self.y1 = self.rand.draw(|g| g.drand()) as f32;
            self.y2 = 0.0;
        }
        for (i, s) in output.iter_mut().enumerate() {
            let chaos = at(inputs[0], i);
            let y0 = (self.y1 * chaos - self.y2 - 0.05).abs();
            self.y2 = self.y1;
            self.y1 = y0;
            *s = y0;
        }
    }
}

/// Which dynamic noise an [`LfdNoise`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LfdShape {
    /// `LFDNoise0`: each value held.
    Step,
    /// `LFDClipNoise`: held, only +/-1.
    Clip,
    /// `LFDNoise1`: a line from one value to the next.
    Linear,
    /// `LFDNoise3`: a cubic through four values, drawn within +/-0.8 so its
    /// overshoot stays within +/-1.
    Cubic,
}

/// scsynth's `float` `sc_wrap` into `[lo, hi)`.
#[inline]
fn sc_wrap(x: f32, lo: f32, hi: f32) -> f32 {
    let range = hi - lo;
    let y = if x >= hi {
        x - range
    } else if x < lo {
        x + range
    } else {
        return x;
    };
    if (lo..hi).contains(&y) {
        y
    } else if hi == lo {
        lo
    } else {
        y - range * ((y - lo) / range).floor()
    }
}

/// `LFDNoise0`, `LFDClipNoise`, `LFDNoise1`, `LFDNoise3`: a random value
/// `freq` times a second, held, joined by lines or by a cubic -- the
/// **dynamic** forms of the `LFNoise` family. Their position in the segment
/// is a phase, recomputed every sample, so a change of `freq` takes effect at
/// once rather than at the next value; the price is a phase per sample, which
/// is why the `LFNoise` forms stay for a fixed rate.
///
/// scsynth's, its phase in `f32` so the values fall on its samples, and its
/// draws in its order. Its phase only decreases, so a negative frequency
/// freezes it; here the rate is the frequency's magnitude.
pub struct LfdNoise {
    rand: SynthRand,
    shape: LfdShape,
    phase: f32,
    /// The values: `[a, b, c, d]` for the cubic, `[prev, next, ..]` for the
    /// line, `[level, ..]` for the steps.
    v: [f32; 4],
    started: bool,
}

impl LfdNoise {
    pub fn new(shape: LfdShape, rand: SynthRand) -> Self {
        Self {
            rand,
            shape,
            phase: 0.0,
            v: [0.0; 4],
            started: false,
        }
    }
}

impl UGen for LfdNoise {
    fn reset(&mut self) {
        self.started = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.started {
            self.started = true;
            self.phase = 0.0;
            // The values scsynth's constructor draws.
            self.v = match self.shape {
                LfdShape::Step | LfdShape::Clip => [0.0; 4],
                LfdShape::Linear => [0.0, self.rand.draw(|g| g.frand2()), 0.0, 0.0],
                LfdShape::Cubic => self.rand.draw(|g| {
                    let mut v = [0.0; 4];
                    for x in v.iter_mut() {
                        *x = g.frand2() * 0.8;
                    }
                    v
                }),
            };
        }
        // scsynth's sample duration, rounded to f32 from the double.
        let dur = (1.0 / ctx.sample_rate as f64) as f32;
        let freq = inputs[0];
        for (i, s) in output.iter_mut().enumerate() {
            self.phase -= at(freq, i).abs() * dur;
            if self.phase < 0.0 {
                self.phase = sc_wrap(self.phase, 0.0, 1.0);
                let v = &mut self.v;
                match self.shape {
                    LfdShape::Step => v[0] = self.rand.draw(|g| g.frand2()),
                    LfdShape::Clip => v[0] = self.rand.draw(|g| g.fcoin()),
                    LfdShape::Linear => {
                        v[0] = v[1];
                        v[1] = self.rand.draw(|g| g.frand2());
                    }
                    LfdShape::Cubic => {
                        v.copy_within(1.., 0);
                        v[3] = self.rand.draw(|g| g.frand2()) * 0.8;
                    }
                }
            }
            let v = &self.v;
            *s = match self.shape {
                LfdShape::Step | LfdShape::Clip => v[0],
                LfdShape::Linear => v[1] + self.phase * (v[0] - v[1]),
                LfdShape::Cubic => cubic32(1.0 - self.phase, v[0], v[1], v[2], v[3]),
            };
        }
    }
}

/// scsynth's `cubicinterp`, in `f32` as its is: the four-point Hermite.
#[inline]
pub(crate) fn cubic32(x: f32, y0: f32, y1: f32, y2: f32, y3: f32) -> f32 {
    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * x + c2) * x + c1) * x + c0
}
