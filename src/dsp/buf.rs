//! Buffer-reading UGens (M5): `PlayBuf` and `BufRd`.
//!
//! Both are **mono**: our UGens have a single output, so multichannel
//! buffers are read one channel per UGen via the `chan` input (two UGens
//! with the same inputs stay sample-locked, so a stereo file is two readers).
//! This diverges from scsynth's multi-output PlayBuf/BufRd — documented in
//! `docs/schemas.md`. Both interpolate linearly between frames; neither has
//! a trigger or done action yet.

use crate::dsp::buffer::Buffer;
use crate::dsp::{ProcessCtx, UGen, at};

/// Reads `buf` at fractional frame `pos` (must be within `0..frames`) with
/// linear interpolation; the upper frame wraps when looping, clamps
/// otherwise.
#[inline]
fn read_lin(buf: &Buffer, pos: f64, channel: usize, looping: bool) -> f32 {
    let f0 = pos as usize; // pos >= 0 by contract
    let frac = (pos - f0 as f64) as f32;
    let f1 = if f0 + 1 < buf.frames() {
        f0 + 1
    } else if looping {
        0
    } else {
        f0
    };
    buf.sample(f0, channel) * (1.0 - frac) + buf.sample(f1, channel) * frac
}

/// Self-advancing buffer player. Inputs: 0 buffer index, 1 channel,
/// 2 rate (frames advanced per output sample, so 1.0 plays at the server
/// rate; scale by `buffer_sr / server_sr` to honor the file's pitch),
/// 3 loop flag. Starts at frame 0; without loop it goes silent at the end.
pub struct PlayBuf {
    phase: f64,
    finished: bool,
}

impl PlayBuf {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            finished: false,
        }
    }
}

impl Default for PlayBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for PlayBuf {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let channel = inputs[1][0].max(0.0) as usize;
        let looping = inputs[3][0] != 0.0;
        let Some(buf) = ctx.buffers.get(index).and_then(|b| b.as_deref()) else {
            output.fill(0.0);
            return;
        };
        let frames = buf.frames() as f64;
        for (i, s) in output.iter_mut().enumerate() {
            // The buffer can shrink under us on a swap: re-clamp, don't trust
            // the phase from previous blocks.
            if self.finished || self.phase >= frames || self.phase < 0.0 {
                if looping && frames > 0.0 {
                    self.phase = self.phase.rem_euclid(frames);
                    self.finished = false;
                } else {
                    self.finished = true;
                    *s = 0.0;
                    continue;
                }
            }
            *s = read_lin(buf, self.phase, channel, looping);
            self.phase += at(inputs[2], i) as f64;
        }
    }
}

/// Buffer reader driven by a phase signal in frames. Inputs: 0 buffer
/// index, 1 channel, 2 phase, 3 loop flag (wrap vs clamp out-of-range
/// phases). Stateless; drive it with any signal for scrubbing/wavetables.
pub struct BufRd;

impl UGen for BufRd {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let channel = inputs[1][0].max(0.0) as usize;
        let looping = inputs[3][0] != 0.0;
        let Some(buf) = ctx.buffers.get(index).and_then(|b| b.as_deref()) else {
            output.fill(0.0);
            return;
        };
        let frames = buf.frames() as f64;
        if frames == 0.0 {
            output.fill(0.0);
            return;
        }
        for (i, s) in output.iter_mut().enumerate() {
            let raw = at(inputs[2], i) as f64;
            let pos = if looping {
                raw.rem_euclid(frames)
            } else {
                raw.clamp(0.0, frames - 1.0)
            };
            *s = read_lin(buf, pos, channel, looping);
        }
    }
}
