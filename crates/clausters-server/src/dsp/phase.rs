//! The phase family: everything driven by one accumulating phase -- the
//! band-limited `Saw`/`Pulse`, the deliberately band-*un*limited `LF*` shapes,
//! and `Phasor` -- plus `TransportPos`, which is a phase the *transport* owns
//! rather than one this file accumulates.
//!
//! **One accumulator, in `f64`.** Every UGen here advances a normalized phase in
//! `[0, 1)` by `freq / sample_rate` per sample.
//!
//! What `f64` gives is worth stating precisely, because the obvious answer is
//! wrong and the tests measure it (`tests/offline/oscillators.rs`,
//! `f64_is_for_the_position_not_the_phase`). For the **wrapped phase** it changes
//! nothing audible:
//! the value never leaves `[0, 1)`, so the rounding error per step is about one
//! ulp of 1.0 and random-walks nowhere -- over ten seconds an `f32` phase and an
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
//! builds `Saw`/`Pulse` from a discrete-summation impulse train -- a sine table
//! divided by a cosecant table -- smoothed by a leaky integrator with a `0.999`
//! pole. That costs a division per sample, two tables, and a settling transient
//! plus a residual DC droop from the integrator. [`poly_blep`] costs a handful
//! of arithmetic ops only on the samples adjacent to a discontinuity -- four per
//! cycle -- and has no state, no tables and no DC error.
//!
//! Its honest cost is that it stays *quasi*-band-limited: the correction is a
//! polynomial approximation of the band-limited step, so a residual remains and
//! grows with the fundamental. The correction here is **fourth order**, spanning
//! two samples on each side of the discontinuity rather than one -- the measured
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
//! the harness can see. `tests/offline/oscillators.rs` regenerates both columns on every
//! run -- the naive baseline is computed there, not hardcoded, so the comparison
//! stays honest if either side changes.
//!
//! **The `LF*` shapes are not band-limited, on purpose**, exactly as in scsynth:
//! they are modulation sources, meant to be read at control rate and to have
//! exact corners.

use crate::dsp::{ProcessCtx, TransportCtx, UGen, at};

/// The fourth-order PolyBLEP residual over `x`, the distance from the
/// discontinuity **in samples**, for a step of height 2 (the range of every
/// waveform here).
///
/// Derived rather than tabulated. A BLEP residual is
/// `2*(integral of K - H)`: the running integral of a smoothing kernel `K` minus the ideal
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
/// antisymmetric, and jumps by exactly `-2` across `x = 0` -- that jump is what
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
/// is about to emit and then moves on -- which is what keeps the first output
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
    fn reset(&mut self) {
        *self = Self::new();
    }

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
/// Built as a naive square with a PolyBLEP correction at each of its two edges --
/// equivalent to the difference of two phase-shifted band-limited saws, but
/// without building either. Width is clamped away from `0` and `1`, where
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
    fn reset(&mut self) {
        *self = Self::new();
    }

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
/// band-limited -- that is the point of the family.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LfShape {
    /// Rising ramp in `[-1, 1]`, starting at `0` for an initial phase of `0`.
    Saw,
    /// Square in `[0, 1]` (scsynth's range for `LFPulse` -- a gate, not a
    /// bipolar waveform like [`Pulse`]), duty cycle from input 2.
    Pulse,
    /// Triangle in `[-1, 1]`, starting at `0` and rising.
    Tri,
    /// scsynth's `VarSaw`: a triangle whose peak sits at the duty point from
    /// input 2, so it sweeps from a falling ramp through a triangle to a rising
    /// one.
    VarSaw,
    /// scsynth's `LFCub`: two cubic arcs, `z^2 (6 - 4z) - 1` up and down, a
    /// sine-like shape in `[-1, 1]` whose slope is continuous. Starts at `0`
    /// rising, as `LFTri` does.
    Cub,
    /// scsynth's `LFPar`: two parabolas, `1 - z^2` and `z^2 - 1`, a cosine-like
    /// shape in `[-1, 1]` starting at its peak.
    Par,
}

/// The non-band-limited modulation shapes: `LFSaw`, `LFPulse`, `LFTri`,
/// `VarSaw`, `LFCub`, `LFPar`. Inputs: 0 frequency in Hz, 1 initial phase, and -- only for the
/// shapes that have a duty cycle -- 2 width. A shape without one declares two
/// inputs rather than three: a UGen that advertises an input it ignores lies to
/// `/ugen_query`, and a client palette would draw an inlet that does nothing.
///
/// **Initial phase is in cycles, `[0, 1)`** -- a deliberate deviation from
/// scsynth, whose `iphase` is in `[0, 2)` for the `LF*` family because its
/// accumulator happens to run over `[-1, 1]`. Exposing an implementation detail
/// as a unit is exactly the kind of wart this project does not inherit; every
/// phase this crate exposes is in cycles.
///
/// The initial phase is read **once**, at the first sample, and ignored after --
/// it names where the oscillator starts, not a running offset.
pub struct Lf {
    shape: LfShape,
    phase: Phase,
    /// The raw phase of the two shapes with a duty cycle, which wrap the way
    /// scsynth's do: at the sample where the phase reaches a cycle, so the
    /// width can be taken there.
    cycle: f64,
    /// The width in force this cycle (`LFPulse`, `VarSaw`).
    duty: f64,
    started: bool,
}

impl Lf {
    pub fn new(shape: LfShape) -> Self {
        Self {
            shape,
            phase: Phase(0.0),
            cycle: 0.0,
            duty: 0.5,
            started: false,
        }
    }

    /// The width a cycle takes: `VarSaw` keeps scsynth's margins, so the
    /// ramp's two slopes stay finite.
    #[inline]
    fn duty_of(&self, w: f32) -> f64 {
        match self.shape {
            LfShape::VarSaw => (w as f64).clamp(0.001, 0.999),
            _ => w as f64,
        }
    }

    #[inline]
    fn value(&self, t: f64) -> f64 {
        match self.shape {
            // Half a cycle in, so an initial phase of 0 emits 0 and rises.
            LfShape::Saw => {
                let u = t + 0.5;
                2.0 * (u - u.floor()) - 1.0
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
            LfShape::Pulse => {
                if t < self.duty {
                    1.0
                } else {
                    0.0
                }
            }
            LfShape::Cub => {
                // scsynth's phase runs over [0, 2) from half a unit in, a
                // quarter cycle: the triangle `z` from there, then the cubic.
                let u = t + 0.25;
                let u = 2.0 * (u - u.floor());
                let z = if u < 1.0 { u } else { 2.0 - u };
                z * z * (6.0 - 4.0 * z) - 1.0
            }
            LfShape::Par => {
                // A cycle is four of scsynth's units: down from the peak over
                // the first, the trough's parabola over the next two, back up.
                let u = 4.0 * t;
                if u < 1.0 {
                    1.0 - u * u
                } else if u < 3.0 {
                    let z = u - 2.0;
                    z * z - 1.0
                } else {
                    let z = u - 4.0;
                    1.0 - z * z
                }
            }
            LfShape::VarSaw => {
                // Rises over the first `width` of the cycle, falls over the
                // rest.
                let w = self.duty;
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
    fn reset(&mut self) {
        *self = Self::new(self.shape);
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        /// What the shapes with no duty cycle see: the value never read.
        const NO_WIDTH: &[f32] = &[0.5];
        let (freq, iphase) = (inputs[0], inputs[1]);
        let width = inputs.get(2).copied().unwrap_or(NO_WIDTH);
        let sr = ctx.sample_rate;
        if !self.started {
            let p = at(iphase, 0) as f64;
            self.phase = Phase(p - p.floor());
            self.cycle = p - p.floor();
            self.duty = self.duty_of(at(width, 0));
            self.started = true;
        }
        let const_dt = block_dt(freq, sr);
        match self.shape {
            LfShape::Saw | LfShape::Tri | LfShape::Cub | LfShape::Par => {
                for (i, s) in output.iter_mut().enumerate() {
                    let dt = const_dt.unwrap_or_else(|| at(freq, i) as f64 / sr as f64);
                    let t = self.phase.advance(dt);
                    *s = self.value(t) as f32;
                }
            }
            LfShape::Pulse | LfShape::VarSaw => {
                // scsynth's: the width is taken once per cycle, at the sample
                // the phase wraps on, so modulating it never cuts a cycle
                // short; and a pulse's wrap sample is always the polarity
                // that would otherwise be missing, so a duty narrower than a
                // sample still pulses.
                for (i, s) in output.iter_mut().enumerate() {
                    let dt = const_dt.unwrap_or_else(|| at(freq, i) as f64 / sr as f64);
                    let z = if self.cycle >= 1.0 || self.cycle < 0.0 {
                        self.cycle -= self.cycle.floor();
                        self.duty = self.duty_of(at(width, i));
                        match self.shape {
                            LfShape::Pulse if self.duty <= 0.5 => 1.0,
                            LfShape::Pulse => 0.0,
                            _ => self.value(self.cycle),
                        }
                    } else {
                        self.value(self.cycle)
                    };
                    *s = z as f32;
                    self.cycle += dt;
                }
            }
        }
    }
}

/// scsynth's `SyncSaw`: a sawtooth at `saw_freq` restarted at every cycle of
/// a master at `sync_freq` -- hard sync, the classic sweep of a slave
/// oscillator's harmonics. Inputs 0 `sync_freq`, 1 `saw_freq`. Output in
/// `[-1, 1)`, starting at `0`.
///
/// scsynth's, including the restart's sub-sample placement: on the sample
/// the master wraps, the slave starts again from where it would be had the
/// restart fallen at the master's exact crossing, the part of the sample past
/// it scaled to the slave's rate. Not band-limited, as scsynth's is not.
pub struct SyncSaw {
    master: f64,
    slave: f64,
}

impl SyncSaw {
    pub fn new() -> Self {
        Self {
            master: 0.0,
            slave: 0.0,
        }
    }
}

impl Default for SyncSaw {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for SyncSaw {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        // scsynth's units: both phases run over [-1, 1), two per cycle.
        let mul = 2.0 / ctx.sample_rate as f64;
        let (sync, saw) = (inputs[0], inputs[1]);
        for (i, s) in output.iter_mut().enumerate() {
            let f1 = at(sync, i) as f64 * mul;
            let f2 = at(saw, i) as f64 * mul;
            *s = self.slave as f32;
            self.slave += f2;
            if self.slave >= 1.0 {
                self.slave -= 2.0;
            }
            self.master += f1;
            if self.master >= 1.0 {
                self.master -= 2.0;
                self.slave = (self.master + 1.0) * f2 / f1 - 1.0;
            }
        }
    }
}

/// scsynth's `Phasor`: a ramp from `start` to `end` advancing by `rate` **per
/// sample**, wrapping at `end`, and jumping to `reset_pos` on a trigger.
///
/// Inputs: 0 trigger, 1 rate, 2 start, 3 end, 4 reset position. `rate` is in
/// output units per sample, not Hz -- that is scsynth's contract and the reason
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
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let _ = ctx;
        let (trig, rate) = (inputs[0], inputs[1]);
        let (start, end, reset) = (inputs[2], inputs[3], inputs[4]);
        if !self.started {
            self.pos = start[0] as f64;
            self.started = true;
        }
        if trig.len() == 1 && output.len() > 1 {
            // A control-rate trigger on an audio-rate ramp resets at the
            // slice's first sample.
            let t = trig[0];
            if t > 0.0 && self.prev_trig <= 0.0 {
                self.pos = reset[0] as f64;
            }
            self.prev_trig = t;
            for (i, s) in output.iter_mut().enumerate() {
                self.pos = sc_wrap(self.pos, at(start, i) as f64, at(end, i) as f64);
                *s = self.pos as f32;
                self.pos += at(rate, i) as f64;
            }
            return;
        }
        for (i, s) in output.iter_mut().enumerate() {
            let r = at(rate, i) as f64;
            // Rising edge, the trigger convention the whole catalog shares,
            // placed between the two samples it crossed between: the ramp
            // has already run the part of a sample since the crossing, as
            // scsynth's does.
            let t = at(trig, i);
            if t > 0.0 && self.prev_trig <= 0.0 {
                let frac = 1.0 - self.prev_trig / (t - self.prev_trig);
                self.pos = at(reset, i) as f64 + frac as f64 * r;
            }
            self.prev_trig = t;
            *s = self.pos as f32;
            self.pos = sc_wrap(self.pos + r, at(start, i) as f64, at(end, i) as f64);
        }
    }
}

/// scsynth's `sc_wrap`: `x` wrapped into `[lo, hi)`, with a single
/// subtraction for the common case of one range out, and `lo` when the range
/// is empty.
#[inline]
pub(crate) fn sc_wrap(x: f64, lo: f64, hi: f64) -> f64 {
    let range = hi - lo;
    let y = if x >= hi {
        x - range
    } else if x < lo {
        x + range
    } else {
        return x;
    };
    if (lo..hi).contains(&y) {
        return y;
    }
    if hi == lo {
        return lo;
    }
    y - range * ((y - lo) / range).floor()
}

/// The **transport's position**, as a signal -- what a buffer
/// reader follows so that seeking, looping and pausing are the transport's
/// and not each reader's.
///
/// One input, `offset` in frames, subtracted from the position: a clip that
/// starts at sample `offset` of the transport reads its own samples from frame 0
/// when the transport reaches it. That is what the input is *for*, but it is
/// also what keeps the output precise -- see below.
///
/// It ramps by one frame per sample while the transport rolls and **holds**
/// while it is stopped, at whatever rate it runs (one `kr` sample covers a
/// whole slice, so the step is the slice's length). A node inside the governed
/// group is frozen and does not run at all while the transport is stopped, so
/// the holding case is for a reader outside it -- or on a server with no group
/// bound, where nothing freezes.
///
/// **Precision.** A signal is `f32`, which represents every integer exactly up
/// to 2^24 -- about 5.8 minutes at 48 kHz -- and in steps of 2 for a while after
/// that. This is the catalog's existing arithmetic (`Phasor` keeps an `f64`
/// internally and outputs `f32` just the same, and `BufRd` takes its phase as
/// a signal), and the `offset` input is the answer to it: the subtraction
/// happens here in `f64`, so a reader positioned at its own buffer's start
/// sees small numbers however deep into a long take it sits. A graph that
/// subtracts with a `Sub` UGen instead has already lost the precision by the
/// time it does.
pub struct TransportPos;

impl UGen for TransportPos {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let base = ctx.transport.position as f64;
        // Frames per output sample: 1 at `ar`, the slice length at `kr`, with
        // no branch on the rate -- the same derivation every timed UGen makes
        // from its own rate against the engine's.
        let step = if ctx.sample_rate > 0.0 {
            (ctx.full_sample_rate / ctx.sample_rate) as f64
        } else {
            1.0
        };
        let rolling = ctx.transport.rolling;
        for (i, s) in output.iter_mut().enumerate() {
            let played = if rolling { i as f64 * step } else { 0.0 };
            *s = (base + played - at(inputs[0], i) as f64) as f32;
        }
    }
}

/// **The transport's declick level**: `1` while it rolls, `0` while it is
/// stopped, and a straight ramp between the two across a stop and a play.
///
/// A stop cannot fade what it has already frozen, so a transport with a ramp
/// (`/transport_fade`) stops in two steps: its governed group goes on running
/// and its position goes on advancing while this falls to zero, and only then
/// does it freeze. A play thaws it and this rises from zero. So an output
/// **outside** the governed group -- in a group that follows the transport --
/// multiplies what the readers wrote by this, and neither edge is a step. A
/// loop's wrap and a locate are not stops and do not move it. With no ramp
/// set, it is simply whether the transport rolls.
///
/// No inputs. It reads the transport of the nearest bound group above it.
pub struct TransportFade;

impl UGen for TransportFade {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        let step = if ctx.sample_rate > 0.0 {
            ctx.full_sample_rate / ctx.sample_rate
        } else {
            1.0
        };
        let TransportCtx {
            fade, fade_step, ..
        } = ctx.transport;
        for (i, s) in output.iter_mut().enumerate() {
            *s = (fade + fade_step * i as f32 * step).clamp(0.0, 1.0);
        }
    }
}
