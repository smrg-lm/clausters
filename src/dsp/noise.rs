//! Noise sources: the stochastic generators, all drawing from the same
//! seeded stream.
//!
//! **Every generator here takes its randomness from `clausters_core::rng`**,
//! the xorshift the sequencing layer and the client's `Pwhite` already use, and
//! every one of them is built from an explicit seed — there is no seedless
//! constructor, and no process-global counter behind one. The seed comes from
//! the instance's [`crate::dsp::registry::BuildCtx`], which reserves a
//! contiguous run per synth, so a render is reproducible: the same score and
//! the same starting seed replay the same samples, which is what lets a noisy
//! patch have a golden file at all. What is *not* in the core is the
//! shaping — the dice table, the random walk, the interpolation — so a client
//! that wanted to draw a pink stream itself would need that moved over first.
//! Only `WhiteNoise`'s generator is mirrored there today.
//!
//! The families:
//!
//! - **Spectral shapes** — `WhiteNoise` (flat), `PinkNoise` (−3 dB/octave),
//!   `BrownNoise` (−6). Each is measured, not asserted by construction.
//! - **Bit and sign sources** — `GrayNoise` (one random bit flipped per
//!   sample), `ClipNoise` (±1 only).
//! - **Held and interpolated** — `LFNoise0`/`LFNoise1`/`LFNoise2` and
//!   `LFClipNoise`: a new random value every `1/freq` seconds, held, ramped or
//!   curved between. These are modulation sources, deliberately not band
//!   limited, like the `LF*` oscillators.
//! - **Impulsive and chaotic** — `Dust`/`Dust2` (random impulses at a mean
//!   density) and `Crackle` (a chaotic map, not a random process at all).

use clausters_core::rng;

use crate::dsp::{ProcessCtx, UGen, at};

/// White noise in [-1, 1], xorshift per sample. No inputs. The generator
/// itself is `clausters_core::rng::WhiteNoise`, so a client can reproduce the
/// stream from the same seed; only the per-instance seeding lives here.
pub struct WhiteNoise {
    noise: rng::WhiteNoise,
}

impl WhiteNoise {
    pub fn with_seed(seed: u64) -> Self {
        Self {
            noise: rng::WhiteNoise::from_seed(seed),
        }
    }
}

impl UGen for WhiteNoise {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        self.noise.fill(output);
    }
}

/// Number of generators in the Voss–McCartney sum. Sixteen covers about five
/// decades — from a period of 2 samples to one of 2^16 — which is the whole
/// audible band and then some at any sample rate we run at.
const PINK_ROWS: usize = 16;

/// `PinkNoise`: equal energy per octave, −3 dB/octave. No inputs.
///
/// **Voss–McCartney**, and deliberately not Trammell's stochastic variant. Both
/// sum a set of white generators updated at halving rates; the difference is
/// the schedule. Voss–McCartney re-rolls the generator picked by the number of
/// trailing zeros in a counter, so **exactly one** of them changes per sample —
/// a fixed cost, every sample, forever. Trammell's version decides at random
/// which rows to update, which is cheaper on average and unbounded in the worst
/// case. An audio callback is not paid on average: it has one block's budget
/// and a run of expensive samples inside it is a dropout. The deterministic
/// schedule is worth more here than the average saving.
///
/// The output is the sum of the rows plus one fresh white sample, mapped
/// linearly onto [-1, 1). Its *peak* therefore reaches ±1 only when all
/// seventeen agree, so like scsynth's it is a quiet signal — around 0.13 RMS
/// against white noise's 0.58. That is the level a ported def expects.
pub struct PinkNoise {
    rng: rng::Rng,
    rows: [f32; PINK_ROWS],
    total: f32,
    counter: u32,
}

impl PinkNoise {
    pub fn with_seed(seed: u64) -> Self {
        let mut rng = rng::Rng::from_seed(seed);
        let mut rows = [0.0f32; PINK_ROWS];
        let mut total = 0.0;
        for r in rows.iter_mut() {
            *r = rng.next_f64() as f32;
            total += *r;
        }
        Self {
            rng,
            rows,
            total,
            counter: 0,
        }
    }
}

impl UGen for PinkNoise {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        for s in output.iter_mut() {
            self.counter = self.counter.wrapping_add(1);
            // Which row to re-roll: the number of trailing zeros doubles the
            // interval at each step, so row k updates every 2^k samples. That
            // *is* the octave spacing the spectrum comes from.
            let k = (self.counter.trailing_zeros() as usize) & (PINK_ROWS - 1);
            let fresh = self.rng.next_f64() as f32;
            self.total += fresh - self.rows[k];
            self.rows[k] = fresh;
            // Rows + one white sample: 17 uniforms in [0, 1), mean 8.5.
            let sum = self.total + self.rng.next_f64() as f32;
            *s = sum * (2.0 / (PINK_ROWS + 1) as f32) - 1.0;
        }
    }
}

/// The largest step a [`BrownNoise`] takes per sample, as scsynth's does. It
/// sets how fast the walk can travel, and with the reflection below it is the
/// whole of the algorithm.
const BROWN_STEP: f64 = 0.125;

/// `BrownNoise`: a random walk, −6 dB/octave. No inputs.
///
/// The walk **reflects** at ±1 rather than clamping. Clamping would let the
/// signal rest against a rail — a constant, which is a click on the way in and
/// silence while it lasts; reflecting keeps it moving and keeps the
/// distribution flat instead of piling probability up at the ends.
pub struct BrownNoise {
    rng: rng::Rng,
    z: f64,
}

impl BrownNoise {
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: rng::Rng::from_seed(seed),
            z: 0.0,
        }
    }
}

impl UGen for BrownNoise {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        for s in output.iter_mut() {
            self.z += self.rng.uniform(-BROWN_STEP, BROWN_STEP);
            if self.z > 1.0 {
                self.z = 2.0 - self.z;
            } else if self.z < -1.0 {
                self.z = -2.0 - self.z;
            }
            *s = self.z as f32;
        }
    }
}

/// `GrayNoise`: one randomly chosen bit of a 32-bit word flipped per sample,
/// the word read as the output. No inputs.
///
/// Two things about it are easy to get wrong. Its **spectrum is not flat**: the
/// high bits flip rarely — one sample in 32 for the top one — and the low bits
/// carry almost no weight, so the energy leans low, measured at −2.9 dB/octave,
/// near enough pink. And its **distribution** is what the kind is really for:
/// consecutive samples differ by exactly one power of two, so the steps span
/// every order of magnitude (the mean step is some four thousand times the
/// median, against 1.14 for white noise) and it sounds grainy rather than
/// smooth. That bit-level property is exact in the **integer** — bit 31 is the
/// sign bit, which is what makes the output bipolar — but it is not recoverable
/// from the output, because `word / 2^31` in `f32` has a 24-bit significand
/// against the word's 31 and rounds by an amount that depends on the magnitude
/// the flip just changed.
pub struct GrayNoise {
    rng: rng::Rng,
    word: i32,
}

impl GrayNoise {
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: rng::Rng::from_seed(seed),
            word: 0,
        }
    }
}

impl UGen for GrayNoise {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        for s in output.iter_mut() {
            self.word ^= 1i32 << (self.rng.next_u64() & 31);
            *s = self.word as f32 * (1.0 / 2_147_483_648.0);
        }
    }
}

/// `ClipNoise`: -1 or 1, nothing between. No inputs.
///
/// A coin flip per sample. Its spectrum is flat like white noise's, but every
/// sample is at full scale, so it is the loudest noise available at a given
/// peak — which is the reason to reach for it and the reason to be careful.
pub struct ClipNoise {
    rng: rng::Rng,
}

impl ClipNoise {
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: rng::Rng::from_seed(seed),
        }
    }
}

impl UGen for ClipNoise {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        for s in output.iter_mut() {
            *s = if self.rng.next_u64() & 1 == 0 {
                -1.0
            } else {
                1.0
            };
        }
    }
}

/// How a [`LfNoise`] gets from one random value to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LfNoiseShape {
    /// `LFNoise0`: hold the value for the whole segment (a step).
    Step,
    /// `LFClipNoise`: hold, but the values are only ±1.
    Clip,
    /// `LFNoise1`: ramp linearly to the next value.
    Linear,
    /// `LFNoise2`: a quadratic through the midpoints, so the *slope* is
    /// continuous too and there is no corner at a segment boundary.
    Quadratic,
}

/// `LFNoise0`, `LFClipNoise`, `LFNoise1` and `LFNoise2`: a new random value
/// every `1/freq` seconds. Input 0 `freq`.
///
/// Not band limited, and deliberately so — like the `LF*` oscillators these are
/// modulation shapes. A step is a step, and asking for one at audio rate is
/// asking for its harmonics.
///
/// The segment length is recomputed **only at a segment boundary**, from the
/// frequency at that moment. So a modulated `freq` changes how long the *next*
/// segment lasts and never stretches the one already running, which would make
/// the value jump.
pub struct LfNoise {
    rng: rng::Rng,
    shape: LfNoiseShape,
    /// Samples left in the current segment.
    counter: i64,
    level: f64,
    slope: f64,
    curve: f64,
    /// `Quadratic` only: the value drawn for the *next* segment, needed to know
    /// where this one's midpoint is.
    next_value: f64,
    primed: bool,
}

impl LfNoise {
    pub fn with_seed(shape: LfNoiseShape, seed: u64) -> Self {
        Self {
            rng: rng::Rng::from_seed(seed),
            shape,
            counter: 0,
            level: 0.0,
            slope: 0.0,
            curve: 0.0,
            next_value: 0.0,
            primed: false,
        }
    }

    /// A fresh value in the shape's own range.
    #[inline]
    fn draw(&mut self) -> f64 {
        match self.shape {
            LfNoiseShape::Clip => {
                if self.rng.next_u64() & 1 == 0 {
                    -1.0
                } else {
                    1.0
                }
            }
            _ => self.rng.uniform(-1.0, 1.0),
        }
    }

    /// Segment length in samples, never shorter than two — a one-sample
    /// segment has no interior for the interpolating shapes to interpolate
    /// over, and the curve's denominator would collapse.
    #[inline]
    fn segment(freq: f32, sr: f32) -> i64 {
        ((sr / freq.abs().max(0.001)) as i64).max(2)
    }
}

impl UGen for LfNoise {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate;
        if !self.primed {
            self.primed = true;
            self.level = self.draw();
            self.next_value = self.draw();
        }
        for (i, s) in output.iter_mut().enumerate() {
            if self.counter <= 0 {
                let n = Self::segment(at(inputs[0], i), sr);
                self.counter = n;
                let value = self.next_value;
                self.next_value = self.draw();
                match self.shape {
                    LfNoiseShape::Step | LfNoiseShape::Clip => self.level = value,
                    LfNoiseShape::Linear => self.slope = (value - self.level) / n as f64,
                    LfNoiseShape::Quadratic => {
                        // Aim at the midpoint between this value and the next,
                        // which is what makes consecutive segments meet with
                        // the same slope. The curve that lands there after `n`
                        // steps of `slope += curve; level += slope` follows
                        // from summing that recurrence.
                        let midpoint = 0.5 * (value + self.next_value);
                        let n = n as f64;
                        self.curve = 2.0 * (midpoint - self.level - n * self.slope) / (n * n + n);
                    }
                }
            }
            self.counter -= 1;
            *s = self.level as f32;
            match self.shape {
                LfNoiseShape::Step | LfNoiseShape::Clip => {}
                LfNoiseShape::Linear => self.level += self.slope,
                LfNoiseShape::Quadratic => {
                    self.slope += self.curve;
                    self.level += self.slope;
                }
            }
        }
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
/// The amplitudes are random too — the trial's own value, rescaled — which is
/// scsynth's behaviour and worth knowing before using `Dust` as a clock.
pub struct Dust {
    rng: rng::Rng,
    mode: DustMode,
}

impl Dust {
    pub fn with_seed(mode: DustMode, seed: u64) -> Self {
        Self {
            rng: rng::Rng::from_seed(seed),
            mode,
        }
    }
}

impl UGen for Dust {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let density = inputs[0];
        let sr = ctx.sample_rate as f64;
        // Block-level fast path: a scalar density means one division per block
        // rather than one per sample.
        let const_thresh = (density.len() == 1).then(|| density[0] as f64 / sr);
        for (i, s) in output.iter_mut().enumerate() {
            let thresh = const_thresh.unwrap_or_else(|| at(density, i) as f64 / sr);
            let z = self.rng.next_f64();
            *s = if thresh > 0.0 && z < thresh {
                match self.mode {
                    DustMode::Unipolar => (z / thresh) as f32,
                    DustMode::Bipolar => (2.0 * z / thresh - 1.0) as f32,
                }
            } else {
                0.0
            };
        }
    }
}

/// `Crackle(chaos)`: the chaotic map `y[n] = |chaos·y[n-1] − y[n-2] − 0.05|`.
/// Input 0 `chaos`.
///
/// **Not a random process.** It has no RNG at all: it is deterministic, and the
/// same `chaos` always gives the same signal from the same start. What makes it
/// noise-like is that the orbit does not close — measured here, no period up to
/// 512 samples — and that `chaos` changes the signal drastically and **not
/// monotonically**: the spread runs 0.56, 0.20, 0.08, 0.05, 0.19, 0.05, 0.06
/// across chaos 0.3 to 1.9. It is a map, not a level control, and the way to
/// use it is by ear. Its output is
/// **unipolar** (the absolute value is part of the map), so it carries DC;
/// subtract its mean or pass it through `LeakDC` before summing it into a bus.
pub struct Crackle {
    y1: f64,
    y2: f64,
}

impl Default for Crackle {
    fn default() -> Self {
        // scsynth's start point. Zero for both would be a fixed point of the
        // map with `chaos` at its default, i.e. a constant.
        Self { y1: 0.3, y2: 0.0 }
    }
}

impl UGen for Crackle {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, s) in output.iter_mut().enumerate() {
            let chaos = at(inputs[0], i) as f64;
            let y0 = (chaos * self.y1 - self.y2 - 0.05).abs();
            self.y2 = self.y1;
            self.y1 = y0;
            *s = y0 as f32;
        }
    }
}
