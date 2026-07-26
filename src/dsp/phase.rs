//! The phase family (U1): everything driven by one accumulating phase — the
//! band-limited `Saw`/`Pulse`, the deliberately band-*un*limited `LF*` shapes,
//! and `Phasor`.
//!
//! **One accumulator, in `f64`.** Every UGen here advances a normalized phase in
//! `[0, 1)` by `freq / sample_rate` per sample.
//!
//! What `f64` buys is worth stating precisely, because the obvious answer is
//! wrong and the tests measure it (`tests/oscillators.rs`,
//! `f64_is_for_the_position_not_the_phase`). For the **wrapped phase** it buys
//! nothing audible:
//! the value never leaves `[0, 1)`, so the rounding error per step is about one
//! ulp of 1.0 and random-walks nowhere — over ten seconds an `f32` phase and an
//! `f64` one both read 55.0003 Hz for a 55 Hz saw. The precision is for
//! [`Phasor`], whose position is **not** wrapped into a small range: as a buffer
//! index eight minutes into a 48 kHz file it is past 2^24, where consecutive
//! `f32` values are 2 apart and `pos += 1.0` rounds back to where it started.
//! Measured, an `f32` position advances **0 frames in ten seconds** there. So
//! the family shares one accumulator type, chosen by the row with the widest
//! range rather than by the most common one. (scsynth instead uses a 32-bit
//! *fixed-point* phase, coarser again.)
//!
//! **Band-limiting is PolyBLEP, not a band-limited impulse train.** scsynth
//! builds `Saw`/`Pulse` from a discrete-summation impulse train — a sine table
//! divided by a cosecant table — smoothed by a leaky integrator with a `0.999`
//! pole. That costs a division per sample, two tables, and a settling transient
//! plus a residual DC droop from the integrator. [`poly_blep`] costs a handful
//! of arithmetic ops only on the samples adjacent to a discontinuity — four per
//! cycle — and has no state, no tables and no DC error.
//!
//! Its honest cost is that it stays *quasi*-band-limited: the correction is a
//! polynomial approximation of the band-limited step, so a residual remains and
//! grows with the fundamental. The correction here is **fourth order**, spanning
//! two samples on each side of the discontinuity rather than one — the measured
//! difference over the second-order form is +29 dB at 105 Hz and +10 to +12 dB
//! over the rest of the range, for four polynomial evaluations per cycle instead
//! of two. The figures (alias SNR, 48 kHz, against the same waveform generated
//! naively):
//!
//! | fundamental | `Saw` | naive ramp | `Pulse` | naive square |
//! |---|---|---|---|---|
//! | 105 Hz | 96.7 dB | 30.9 dB | 98.4 dB | 32.7 dB |
//! | 996 Hz | 42.6 dB | 16.0 dB | 43.5 dB | 17.7 dB |
//! | 3996 Hz | 39.2 dB | 9.9 dB | 38.9 dB | 11.4 dB |
//!
//! At 105 Hz that is within about 2.5 dB of the measurement's own floor (a pure
//! tone reads 99.2 dB through the same analysis), so the low end is as clean as
//! the harness can see. `tests/oscillators.rs` regenerates both columns on every
//! run — the naive baseline is computed there, not hardcoded, so the comparison
//! stays honest if either side changes.
//!
//! **The `LF*` shapes are not band-limited, on purpose**, exactly as in scsynth:
//! they are modulation sources, meant to be read at control rate and to have
//! exact corners.

use crate::dsp::{ProcessCtx, UGen, at};

/// The fourth-order PolyBLEP residual over `x`, the distance from the
/// discontinuity **in samples**, for a step of height 2 (the range of every
/// waveform here).
///
/// Derived rather than tabulated. A BLEP residual is
/// `2·(∫K − H)`: the running integral of a smoothing kernel `K` minus the ideal
/// step `H`, scaled to the step's height. Taking `K` to be the **cubic B-spline**
/// (support `[-2, 2]`, unit area) and integrating piecewise gives
///
/// ```text
///  x in [-2,-1]:   (2 + x)^4 / 12
///  x in [-1, 0]:   4x/3 - 2x^3/3 - x^4/4 + 1
///  x in [ 0, 1]:   4x/3 - 2x^3/3 + x^4/4 - 1
///  x in [ 1, 2]:  -(2 - x)^4 / 12
/// ```
///
/// which is continuous at `+/-1` and `+/-2`, vanishes at `+/-2`, is
/// antisymmetric, and jumps by exactly `-2` across `x = 0` — that jump is what
/// cancels the waveform's own. The quadratic two-sample residual is the same
/// construction one order down (`K` the triangular B-spline), and it is what the
/// wide-`dt` fallback below uses.
#[inline]
fn blep4(x: f64) -> f64 {
    let a = x.abs();
    if a >= 2.0 {
        return 0.0;
    }
    if a >= 1.0 {
        let u = 2.0 - a;
        let r = u * u * u * u / 12.0;
        return if x < 0.0 { r } else { -r };
    }
    let x2 = x * x;
    let common = 4.0 * x / 3.0 - 2.0 * (x2 * x) / 3.0;
    let quartic = x2 * x2 / 4.0;
    if x < 0.0 {
        common - quartic + 1.0
    } else {
        common + quartic - 1.0
    }
}

/// The PolyBLEP residual: what to add to a naive waveform to soften a
/// discontinuity that falls between samples.
///
/// `t` is the phase in `[0, 1)` and `dt` the phase increment per sample, so the
/// correction applies to the **two samples on each side** of a wrap and is
/// exactly zero everywhere else. That is what keeps it cheap: the oscillator's
/// inner loop is the naive expression plus one comparison, and the polynomial
/// only runs on four samples per cycle.
///
/// Above `sr/4` the two correction regions would overlap, so the increment is
/// checked once and the calculation drops to the two-sample (quadratic) residual,
/// which stays disjoint up to `sr/2`. The switch is inaudible where it happens:
/// a waveform whose fundamental is above `sr/4` has at most one harmonic left.
///
/// Returns 0 for a non-positive `dt`: a stopped oscillator has no step, and a
/// reversed one passes `|dt|` (see `blep_saw` for why direction cancels).
#[inline]
pub fn poly_blep(t: f64, dt: f64) -> f64 {
    if dt <= 0.0 {
        return 0.0;
    }
    if dt < 0.25 {
        if t < 2.0 * dt {
            return blep4(t / dt);
        }
        if t > 1.0 - 2.0 * dt {
            return blep4((t - 1.0) / dt);
        }
        return 0.0;
    }
    if dt < 0.5 {
        if t < dt {
            let x = t / dt;
            return x + x - x * x - 1.0;
        }
        if t > 1.0 - dt {
            let x = (t - 1.0) / dt;
            return x * x + x + x + 1.0;
        }
    }
    0.0
}

/// A normalized phase accumulator in `[0, 1)`.
///
/// `advance` returns the phase *before* stepping, so a caller reads the value it
/// is about to emit and then moves on — which is what keeps the first output
/// sample equal to the initial phase rather than one increment past it.
#[derive(Clone, Copy)]
struct Phase(f64);

impl Phase {
    #[inline]
    fn advance(&mut self, dt: f64) -> f64 {
        let now = self.0;
        self.0 += dt;
        // A single wrap suffices for |dt| < 1; beyond Nyquist the oscillator is
        // meaningless anyway, and `fract` keeps it finite instead of drifting.
        if self.0 >= 1.0 || self.0 < 0.0 {
            self.0 -= self.0.floor();
        }
        now
    }
}

/// Per-sample phase increment, and whether it is constant over the whole block.
///
/// A `freq` input arriving as a length-1 wire (a constant, or an `ir`/`kr`
/// value) makes the increment block-constant, so the division happens once
/// instead of once per sample. This is the block-level fast path the U track
/// takes everywhere: scsynth reaches the same place by generating a separate
/// `next` function per input-rate combination.
#[inline]
fn block_dt(freq: &[f32], sr: f32) -> Option<f64> {
    (freq.len() == 1).then(|| freq[0] as f64 / sr as f64)
}

/// Band-limited sawtooth, rising, in `[-1, 1]`. Input 0 is the frequency in Hz.
///
/// The phase starts half a cycle in so the **first output sample is 0**: a saw
/// starting at `-1` would inject a step at every note onset, which is both a
/// click and a DC transient through any following filter.
pub struct Saw {
    phase: Phase,
}

impl Saw {
    pub fn new() -> Self {
        Self { phase: Phase(0.5) }
    }
}

impl Default for Saw {
    fn default() -> Self {
        Self::new()
    }
}

/// The naive ramp plus its correction, shared by [`Saw`] and [`Pulse`].
///
/// **Direction cancels out.** Running the phase backwards reverses two things
/// at once: which side of the discontinuity a sample sits on, and the sign of
/// the jump. The residual is antisymmetric, so the two reversals cancel and the
/// correction is the *same* expression with `|dt|`. Mirroring the phase instead
/// (`1 - t`) is algebraically equivalent but evaluates the polynomial on a
/// difference of nearly equal numbers, which measurably costs precision at
/// fourth order.
#[inline]
fn blep_saw(t: f64, dt: f64) -> f64 {
    2.0 * t - 1.0 - poly_blep(t, dt.abs())
}

impl UGen for Saw {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let freq = inputs[0];
        let sr = ctx.sample_rate;
        match block_dt(freq, sr) {
            Some(dt) => {
                for s in output.iter_mut() {
                    let t = self.phase.advance(dt);
                    *s = blep_saw(t, dt) as f32;
                }
            }
            None => {
                for (i, s) in output.iter_mut().enumerate() {
                    let dt = at(freq, i) as f64 / sr as f64;
                    let t = self.phase.advance(dt);
                    *s = blep_saw(t, dt) as f32;
                }
            }
        }
    }
}

/// Band-limited pulse in `[-1, 1]`. Inputs: 0 frequency in Hz, 1 pulse width as
/// a fraction of the cycle (`0.5` = square).
///
/// Built as a naive square with a PolyBLEP correction at each of its two edges —
/// equivalent to the difference of two phase-shifted band-limited saws, but
/// without materializing either. Width is clamped away from `0` and `1`, where
/// the two edges would coincide and the waveform would collapse to silence with
/// a discontinuity of twice the amplitude.
pub struct Pulse {
    phase: Phase,
}

impl Pulse {
    pub fn new() -> Self {
        Self { phase: Phase(0.0) }
    }
}

impl Default for Pulse {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn blep_pulse(t: f64, dt: f64, width: f64) -> f64 {
    let naive = if t < width { 1.0 } else { -1.0 };
    // The rising edge sits at phase 0, the falling one at `width`; each is
    // corrected around its own position, with opposite signs. Direction cancels
    // out exactly as in `blep_saw`. When the two edges are closer than the
    // correction is wide the residuals simply superpose, which is what a sum of
    // two band-limited steps is anyway.
    let fall = {
        let u = t - width;
        if u < 0.0 { u + 1.0 } else { u }
    };
    let d = dt.abs();
    naive + poly_blep(t, d) - poly_blep(fall, d)
}

impl UGen for Pulse {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (freq, width) = (inputs[0], inputs[1]);
        let sr = ctx.sample_rate;
        match block_dt(freq, sr) {
            Some(dt) => {
                for (i, s) in output.iter_mut().enumerate() {
                    let w = (at(width, i) as f64).clamp(1e-4, 1.0 - 1e-4);
                    let t = self.phase.advance(dt);
                    *s = blep_pulse(t, dt, w) as f32;
                }
            }
            None => {
                for (i, s) in output.iter_mut().enumerate() {
                    let dt = at(freq, i) as f64 / sr as f64;
                    let w = (at(width, i) as f64).clamp(1e-4, 1.0 - 1e-4);
                    let t = self.phase.advance(dt);
                    *s = blep_pulse(t, dt, w) as f32;
                }
            }
        }
    }
}

/// The waveform an [`Lf`] oscillator draws from its phase. None of these is
/// band-limited — that is the point of the family.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LfShape {
    /// Rising ramp in `[-1, 1]`, starting at `0` for an initial phase of `0`.
    Saw,
    /// Square in `[0, 1]` (scsynth's range for `LFPulse` — a gate, not a
    /// bipolar waveform like [`Pulse`]), duty cycle from input 2.
    Pulse,
    /// Triangle in `[-1, 1]`, starting at `0` and rising.
    Tri,
    /// scsynth's `VarSaw`: a triangle whose peak sits at the duty point from
    /// input 2, so it sweeps from a falling ramp through a triangle to a rising
    /// one.
    VarSaw,
}

/// The non-band-limited modulation shapes: `LFSaw`, `LFPulse`, `LFTri`,
/// `VarSaw`. Inputs: 0 frequency in Hz, 1 initial phase, and — only for the
/// shapes that have a duty cycle — 2 width. A shape without one declares two
/// inputs rather than three: a UGen that advertises an input it ignores lies to
/// `/u_query`, and a client palette would draw an inlet that does nothing.
///
/// **Initial phase is in cycles, `[0, 1)`** — a deliberate deviation from
/// scsynth, whose `iphase` is in `[0, 2)` for the `LF*` family because its
/// accumulator happens to run over `[-1, 1]`. Exposing an implementation detail
/// as a unit is exactly the kind of wart this project does not inherit; every
/// phase this crate exposes is in cycles.
///
/// The initial phase is read **once**, at the first sample, and ignored after —
/// it names where the oscillator starts, not a running offset.
pub struct Lf {
    shape: LfShape,
    phase: Phase,
    started: bool,
}

impl Lf {
    pub fn new(shape: LfShape) -> Self {
        Self {
            shape,
            phase: Phase(0.0),
            started: false,
        }
    }

    #[inline]
    fn value(&self, t: f64, width: f64) -> f64 {
        match self.shape {
            // Half a cycle in, so an initial phase of 0 emits 0 and rises.
            LfShape::Saw => {
                let u = t + 0.5;
                2.0 * (u - u.floor()) - 1.0
            }
            LfShape::Pulse => {
                if t < width {
                    1.0
                } else {
                    0.0
                }
            }
            LfShape::Tri => {
                // Starts at 0, up to 1 by a quarter cycle, down to -1 by three
                // quarters, back to 0.
                let u = t * 4.0;
                match u {
                    u if u < 1.0 => u,
                    u if u < 3.0 => 2.0 - u,
                    u => u - 4.0,
                }
            }
            LfShape::VarSaw => {
                // Rises over the first `width` of the cycle, falls over the
                // rest; both limits degenerate to a plain ramp.
                let w = width.clamp(1e-4, 1.0 - 1e-4);
                if t < w {
                    2.0 * t / w - 1.0
                } else {
                    1.0 - 2.0 * (t - w) / (1.0 - w)
                }
            }
        }
    }
}

impl UGen for Lf {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        /// What the shapes with no duty cycle see: the value never read.
        const NO_WIDTH: &[f32] = &[0.5];
        let (freq, iphase) = (inputs[0], inputs[1]);
        let width = inputs.get(2).copied().unwrap_or(NO_WIDTH);
        let sr = ctx.sample_rate;
        if !self.started {
            let p = at(iphase, 0) as f64;
            self.phase = Phase(p - p.floor());
            self.started = true;
        }
        let const_dt = block_dt(freq, sr);
        for (i, s) in output.iter_mut().enumerate() {
            let dt = const_dt.unwrap_or_else(|| at(freq, i) as f64 / sr as f64);
            let w = at(width, i) as f64;
            let t = self.phase.advance(dt);
            *s = self.value(t, w) as f32;
        }
    }
}

/// scsynth's `Phasor`: a ramp from `start` to `end` advancing by `rate` **per
/// sample**, wrapping at `end`, and jumping to `reset_pos` on a trigger.
///
/// Inputs: 0 trigger, 1 rate, 2 start, 3 end, 4 reset position. `rate` is in
/// output units per sample, not Hz — that is scsynth's contract and the reason
/// `Phasor` is the natural index source for a buffer reader (a rate of `1`
/// advances one frame per sample). It is not band-limited and is not meant to
/// be listened to directly.
pub struct Phasor {
    pos: f64,
    prev_trig: f32,
    started: bool,
}

impl Phasor {
    pub fn new() -> Self {
        Self {
            pos: 0.0,
            prev_trig: 0.0,
            started: false,
        }
    }
}

impl Default for Phasor {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Phasor {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let _ = ctx;
        let (trig, rate) = (inputs[0], inputs[1]);
        let (start, end, reset) = (inputs[2], inputs[3], inputs[4]);
        for (i, s) in output.iter_mut().enumerate() {
            let lo = at(start, i) as f64;
            let hi = at(end, i) as f64;
            if !self.started {
                self.pos = lo;
                self.started = true;
            }
            // Rising edge, the trigger convention the whole catalog shares.
            let t = at(trig, i);
            if t > 0.0 && self.prev_trig <= 0.0 {
                self.pos = at(reset, i) as f64;
            }
            self.prev_trig = t;

            *s = self.pos as f32;
            self.pos += at(rate, i) as f64;
            // One wrap expression covers both ends; the test that it is needed
            // at all keeps the `floor` off the common in-range sample. A
            // non-positive range (start >= end) simply holds.
            let range = hi - lo;
            if range > 0.0 && !(lo..hi).contains(&self.pos) {
                self.pos -= range * ((self.pos - lo) / range).floor();
            }
        }
    }
}
