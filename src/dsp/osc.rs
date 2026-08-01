//! Table-reading oscillators and the waveshaper (S5): the first consumers of
//! the wavetable format built by `/buffer_gen` (see [`crate::dsp::wavetable`]).
//!
//! - [`Osc`] — interpolating wavetable oscillator (needs a `wavetable`-format
//!   buffer).
//! - [`OscN`] — non-interpolating oscillator over a plain (non-wavetable)
//!   buffer.
//! - [`VOsc`] — like [`Osc`] but the buffer number is a signal, crossfading
//!   between adjacent wavetables for morphing timbres.
//! - [`Shaper`] — waveshaper: maps its input signal through a `cheby`-style
//!   transfer table (wavetable format).
//!
//! All are mono, single-output, and read the immutable buffer pool through the
//! context — no allocation, like every other UGen.

use crate::dsp::buffer::Buffer;
use crate::dsp::wavetable::wt_interp;
use crate::dsp::{ProcessCtx, UGen, at};

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
fn read_wavetable(table: &[f32], pos: f64) -> f32 {
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
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let Some(buf) = ctx.buffers.get(index).and_then(|b| b.as_deref()) else {
            output.fill(0.0);
            return;
        };
        let table = buf.data();
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
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let Some(buf) = ctx.buffers.get(index).and_then(|b| b.as_deref()) else {
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
/// 0 buffer position, 1 frequency, 2 phase offset in radians.
pub struct VOsc {
    phase: f64,
}

impl VOsc {
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }
}

impl Default for VOsc {
    fn default() -> Self {
        Self::new()
    }
}

impl VOsc {
    /// One wavetable's contribution at `pos`, 0 if the slot is empty.
    #[inline]
    fn read(buffers: &[Option<std::sync::Arc<Buffer>>], index: usize, pos: f64) -> f32 {
        match buffers.get(index).and_then(|b| b.as_deref()) {
            Some(buf) => read_wavetable(buf.data(), pos),
            None => 0.0,
        }
    }
}

impl UGen for VOsc {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        for (i, s) in output.iter_mut().enumerate() {
            let bufpos = at(inputs[0], i).max(0.0);
            let b0 = bufpos as usize;
            let bfrac = bufpos - b0 as f32;
            let phase_off = at(inputs[2], i) as f64 / std::f64::consts::TAU;
            let pos = self.phase + phase_off;
            let v0 = Self::read(ctx.buffers, b0, pos);
            let v1 = Self::read(ctx.buffers, b0 + 1, pos);
            *s = v0 + bfrac * (v1 - v0);
            advance(&mut self.phase, at(inputs[1], i), sr);
        }
    }
}

/// Waveshaper: maps its input signal through a transfer table (wavetable
/// format, typically built by `/buffer_gen cheby`). Input `x` in `[-1, 1]` indexes
/// the table from its first to its last point; values outside are clamped.
/// Inputs: 0 buffer index, 1 input signal. Stateless.
pub struct Shaper;

impl UGen for Shaper {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let Some(buf) = ctx.buffers.get(index).and_then(|b| b.as_deref()) else {
            output.fill(0.0);
            return;
        };
        let table = buf.data();
        let points = table.len() / 2;
        if points == 0 {
            output.fill(0.0);
            return;
        }
        // x in [-1, 1] spans the table's first..last point.
        let span = (points - 1) as f32;
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[1], i).clamp(-1.0, 1.0);
            let scaled = (x * 0.5 + 0.5) * span;
            let k = (scaled as usize).min(points - 1);
            *s = wt_interp(table, k, scaled - k as f32);
        }
    }
}
