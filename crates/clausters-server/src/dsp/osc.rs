//! Table-reading oscillators and the waveshaper: the first consumers of
//! the wavetable format built by `/buffer_gen` (see [`crate::dsp::wavetable`]).
//!
//! - [`Osc`] -- interpolating wavetable oscillator (needs a `wavetable`-format
//!   buffer).
//! - [`OscN`] -- non-interpolating oscillator over a plain (non-wavetable)
//!   buffer.
//! - [`VOsc`] -- like [`Osc`] but the buffer number is a signal, crossfading
//!   between adjacent wavetables for morphing timbres.
//! - [`VOsc3`] -- [`VOsc`] with three frequencies summed.
//! - [`COsc`] -- [`Osc`] twice, detuned by a beat frequency, summed: a chorus.
//! - [`Shaper`] -- waveshaper: maps its input signal through a `cheby`-style
//!   transfer table (wavetable format).
//!
//! All are mono, single-output, and read the immutable buffer pool through the
//! context -- no allocation, like every other UGen.

use crate::dsp::wavetable::wt_interp;
use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// Advances a normalized phase (cycles, `[0, 1)`) by `freq/sr` per sample,
/// keeping it wrapped. Shared by [`Osc`], [`OscN`] and [`VOsc`].
#[inline(always)]
fn advance(phase: &mut f64, freq: f32, sr: f64) {
    *phase += freq as f64 / sr;
    if *phase >= 1.0 || *phase < 0.0 {
        *phase = phase.rem_euclid(1.0);
    }
}

/// Reads a wavetable at normalized position `pos` (cycles, any real) with
/// linear interpolation. `table.len()` is `2 * points`.
#[inline(always)]
fn read_wavetable(table: &[std::sync::atomic::AtomicU32], pos: f64) -> f32 {
    let points = table.len() / 2;
    if points == 0 {
        return 0.0;
    }
    let scaled = pos.rem_euclid(1.0) * points as f64;
    let k = (scaled as usize).min(points - 1);
    wt_interp(table, k, (scaled - k as f64) as f32)
}

/// Interpolating wavetable oscillator. Inputs: 0 buffer index (a `wavetable`-
/// format buffer), 1 frequency in Hz, 2 phase offset in radians. Silent while
/// the buffer is missing or empty.
pub struct Osc {
    phase: f64,
}

impl Osc {
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }
}

impl Default for Osc {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Osc {
    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let Some(buf) = ctx.buffer(index) else {
            output.fill(0.0);
            return;
        };
        // A wavetable is read as a contiguous span, so a join has nothing to
        // offer here: it is a window onto takes and not a table.
        let Some(table) = buf.cells() else {
            output.fill(0.0);
            return;
        };
        let sr = ctx.sample_rate as f64;
        for (i, s) in output.iter_mut().enumerate() {
            let phase_off = at(inputs[2], i) as f64 / std::f64::consts::TAU;
            *s = read_wavetable(table, self.phase + phase_off);
            advance(&mut self.phase, at(inputs[1], i), sr);
        }
    }
}

/// Non-interpolating oscillator over a **plain** buffer (one sample per point,
/// no wavetable format). Inputs: 0 buffer index, 1 frequency, 2 phase offset
/// in radians. Cheaper and rawer than [`Osc`]; good for lo-fi timbres.
pub struct OscN {
    phase: f64,
}

impl OscN {
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }
}

impl Default for OscN {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for OscN {
    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let Some(buf) = ctx.buffer(index) else {
            output.fill(0.0);
            return;
        };
        let frames = buf.frames();
        if frames == 0 {
            output.fill(0.0);
            return;
        }
        let sr = ctx.sample_rate as f64;
        for (i, s) in output.iter_mut().enumerate() {
            let phase_off = at(inputs[2], i) as f64 / std::f64::consts::TAU;
            let pos = (self.phase + phase_off).rem_euclid(1.0) * frames as f64;
            *s = buf.sample((pos as usize).min(frames - 1), 0);
            advance(&mut self.phase, at(inputs[1], i), sr);
        }
    }
}

/// Variable wavetable oscillator: the buffer number is a signal. Reads
/// wavetable `bufpos` and `bufpos + 1` and crossfades by its fractional part,
/// so sweeping `bufpos` morphs between a bank of adjacent tables. Inputs:
/// 0 buffer position, 1 frequency, 2 phase offset in radians. A control-rate
/// `bufpos` is a line across the block, as scsynth's is, so a sweep does not
/// step at the block rate.
pub struct VOsc {
    phase: f64,
    bufpos: Ramp,
}

impl VOsc {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            bufpos: Ramp::default(),
        }
    }
}

impl Default for VOsc {
    fn default() -> Self {
        Self::new()
    }
}

/// One wavetable's contribution at `pos`, 0 if the slot is empty.
#[inline]
fn read_slot(ctx: &ProcessCtx, index: usize, pos: f64) -> f32 {
    match ctx.buffer(index) {
        Some(buf) => buf.cells().map_or(0.0, |t| read_wavetable(t, pos)),
        None => 0.0,
    }
}

/// The table position at sample `i`: the ramp for a control-rate `bufpos`,
/// the sample itself for an audio-rate one.
#[inline]
fn bufpos_at(bufpos: &[f32], line: (f64, f64), i: usize) -> f32 {
    if bufpos.len() == 1 {
        (line.0 + line.1 * i as f64) as f32
    } else {
        bufpos[i]
    }
    .max(0.0)
}

/// The two tables either side of `bufpos` at `pos`, crossfaded by its
/// fraction.
#[inline]
fn morph(ctx: &ProcessCtx, bufpos: f32, pos: f64) -> f32 {
    let b0 = bufpos as usize;
    let frac = bufpos - b0 as f32;
    let v0 = read_slot(ctx, b0, pos);
    let v1 = read_slot(ctx, b0 + 1, pos);
    v0 + frac * (v1 - v0)
}

impl UGen for VOsc {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        let line = self.bufpos.toward(inputs[0][0] as f64, output.len());
        for (i, s) in output.iter_mut().enumerate() {
            let phase_off = at(inputs[2], i) as f64 / std::f64::consts::TAU;
            *s = morph(ctx, bufpos_at(inputs[0], line, i), self.phase + phase_off);
            advance(&mut self.phase, at(inputs[1], i), sr);
        }
    }
}

/// scsynth's `VOsc3(bufpos, freq1, freq2, freq3)`: [`VOsc`]'s morphing table
/// read at three frequencies and summed -- three detuned voices of one
/// timbre for the price of one table position.
pub struct VOsc3 {
    phases: [f64; 3],
    bufpos: Ramp,
}

impl VOsc3 {
    pub fn new() -> Self {
        Self {
            phases: [0.0; 3],
            bufpos: Ramp::default(),
        }
    }
}

impl Default for VOsc3 {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for VOsc3 {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        let line = self.bufpos.toward(inputs[0][0] as f64, output.len());
        for (i, s) in output.iter_mut().enumerate() {
            let pos = bufpos_at(inputs[0], line, i);
            let mut sum = 0.0;
            for (k, phase) in self.phases.iter_mut().enumerate() {
                sum += morph(ctx, pos, *phase);
                advance(phase, at(inputs[1 + k], i), sr);
            }
            *s = sum;
        }
    }
}

/// scsynth's `COsc(bufnum, freq, beats)`: [`Osc`] twice, at `freq + beats/2`
/// and `freq - beats/2`, summed -- two voices beating `beats` times a second,
/// a chorus from one table. Its peak is twice the table's.
pub struct COsc {
    phases: [f64; 2],
}

impl COsc {
    pub fn new() -> Self {
        Self { phases: [0.0; 2] }
    }
}

impl Default for COsc {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for COsc {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let Some(table) = ctx
            .buffers
            .get(index)
            .and_then(|b| b.as_deref())
            .and_then(|b| b.cells())
        else {
            output.fill(0.0);
            return;
        };
        let sr = ctx.sample_rate as f64;
        for (i, s) in output.iter_mut().enumerate() {
            let freq = at(inputs[1], i);
            let half = at(inputs[2], i) * 0.5;
            let [a, b] = &mut self.phases;
            *s = read_wavetable(table, *a) + read_wavetable(table, *b);
            advance(a, freq + half, sr);
            advance(b, freq - half, sr);
        }
    }
}

/// Waveshaper: maps its input signal through a transfer table (wavetable
/// format, typically built by `/buffer_gen cheby`). Input `x` in `[-1, 1]` spans
/// the table's intervals, from its first point to the end of its last pair, as
/// scsynth's does; values outside are clamped.
/// Inputs: 0 buffer index, 1 input signal. Stateless.
pub struct Shaper;

impl UGen for Shaper {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let Some(buf) = ctx.buffer(index) else {
            output.fill(0.0);
            return;
        };
        // Same as `Osc`: a transfer table is a contiguous span.
        let Some(table) = buf.cells() else {
            output.fill(0.0);
            return;
        };
        let points = table.len() / 2;
        if points == 0 {
            output.fill(0.0);
            return;
        }
        // x in [-1, 1] spans the table's `points` intervals, the last ending
        // at x = 1 (scsynth's `ShaperPerform`: the index is kept just short
        // of the end, so x = 1 reads the top of the last pair).
        let n = points as f32;
        let top = n - 0.001;
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[1], i);
            let scaled = (n * 0.5 + x * n * 0.5).clamp(0.0, top);
            let k = (scaled as usize).min(points - 1);
            *s = wt_interp(table, k, scaled - k as f32);
        }
    }
}
