//! Buffer-reading and buffer-**writing** UGens: `PlayBuf` and `BufRd`,
//! `RecordBuf` and `BufWr`, plus the `BufInfo` family that reports a buffer's
//! shape/rate at run time.
//!
//! All four are **mono**: our UGens have a single output, so multichannel
//! buffers are read or written one channel per UGen via the `chan` input (two
//! UGens with the same inputs stay sample-locked, so a stereo file is two
//! readers and two writers). This diverges from scsynth's multi-output
//! PlayBuf/BufRd — documented in `docs/schemas.md`. The two readers interpolate
//! linearly; neither has a trigger or done action yet.
//!
//! **A buffer's contents are mutable** (`dsp::buffer`), so recording into one
//! while another node plays it is the ordinary case rather than a hazard: what
//! a reader crossing the write head sees is some old samples and some new,
//! never half of one, which is what a looper has always sounded like. The
//! writers here take `&self` on the buffer for the same reason everything else
//! does — the pool reaches the audio thread through an `Arc`, and the cells
//! carry the mutability.

use crate::dsp::buffer::Buffer;
use crate::dsp::{DoneAction, ProcessCtx, UGen, at};

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

/// Writes a signal into a buffer at a **phase** in frames — the write-side twin
/// of [`BufRd`], and stateless in the same way. Inputs: 0 buffer index, 1
/// channel, 2 phase, 3 loop flag (wrap vs clamp out-of-range phases), 4 the
/// signal.
///
/// The destination comes first and the signal last, which is the order every
/// writer in this catalog uses (`Out`, `ReplaceOut`, `OutCtl`). Like `OutCtl`
/// it **passes the signal through** as its output, so a chain can go on using
/// what it just recorded without a second wire.
///
/// No interpolation: a write lands on the frame the phase names, truncated.
/// Spreading one sample over two frames — what interpolating on write would
/// mean — writes a value that was never in the signal, and the two neighbours
/// of consecutive writes would fight over the same cells.
pub struct BufWr;

impl UGen for BufWr {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let channel = inputs[1][0].max(0.0) as usize;
        let looping = inputs[3][0] != 0.0;
        let Some(buf) = ctx.buffers.get(index).and_then(|b| b.as_deref()) else {
            // Nowhere to write, but the signal still passes: a missing buffer
            // silences the recording, not the chain.
            for (i, s) in output.iter_mut().enumerate() {
                *s = at(inputs[4], i);
            }
            return;
        };
        let frames = buf.frames() as f64;
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[4], i);
            let raw = at(inputs[2], i) as f64;
            let pos = if looping {
                raw.rem_euclid(frames.max(1.0))
            } else {
                raw
            };
            if pos >= 0.0 && pos < frames {
                buf.set_sample(pos as usize, channel, x);
            }
            *s = x;
        }
    }
}

/// Records a signal into a buffer, advancing one frame per sample — the
/// self-advancing writer, as [`PlayBuf`] is the self-advancing reader.
///
/// Inputs: 0 buffer index, 1 channel, 2 the signal, 3 `offset` (the frame a
/// start or a re-trigger cues to), 4 `rec_level`, 5 `pre_level`, 6 `run`, 7
/// loop flag, 8 `trigger`, 9 `done_action`.
///
/// **`rec_level` and `pre_level` are what make it a looper rather than a tape
/// head**: each frame becomes `in·rec_level + old·pre_level`, so `(1, 0)`
/// overwrites, `(1, 1)` overdubs onto what is there, and `(1, 0.5)` overdubs
/// with the older layers fading — scsynth's own convention, and the reason
/// they are inputs rather than a mode.
///
/// `run` at zero holds the position and writes nothing, so a recording can be
/// gated without losing its place. A rising `trigger` re-cues to `offset`.
/// Without `loop`, reaching the end stops the recording and fires the done
/// action; with it, the position wraps and recording never ends.
///
/// The output is the input, passed through, for the reason [`BufWr`]'s is.
pub struct RecordBuf {
    /// Next frame to write. `usize` rather than a phase: a recorder advances
    /// exactly one frame per sample, and there is no fractional position for
    /// it to be at.
    pos: usize,
    started: bool,
    finished: bool,
    prev_trig: f32,
    done_action: DoneAction,
}

impl RecordBuf {
    pub fn new() -> Self {
        Self {
            pos: 0,
            started: false,
            finished: false,
            prev_trig: 0.0,
            done_action: DoneAction::None,
        }
    }
}

impl Default for RecordBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for RecordBuf {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let channel = inputs[1][0].max(0.0) as usize;
        let looping = inputs[7][0] != 0.0;
        let offset = inputs[3][0].max(0.0) as usize;
        // Read like `EnvGen`'s: block-scalar, so a def can pick the action from
        // a control and a client can change it before the recording ends.
        self.done_action = DoneAction::from_i32(at(inputs[9], 0) as i32);
        if !self.started {
            self.pos = offset;
            self.started = true;
        }
        let Some(buf) = ctx.buffers.get(index).and_then(|b| b.as_deref()) else {
            for (i, s) in output.iter_mut().enumerate() {
                *s = at(inputs[2], i);
            }
            return;
        };
        let frames = buf.frames();
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[2], i);
            *s = x;
            // The trigger is read whatever else is happening: re-cueing a
            // finished or paused recorder is how one is restarted.
            let t = at(inputs[8], i);
            if t > 0.0 && self.prev_trig <= 0.0 {
                self.pos = offset;
                self.finished = false;
            }
            self.prev_trig = t;
            if self.finished || at(inputs[6], i) <= 0.0 || frames == 0 {
                continue;
            }
            if self.pos >= frames {
                if looping {
                    self.pos %= frames;
                } else {
                    self.finished = true;
                    continue;
                }
            }
            let old = buf.sample(self.pos, channel);
            buf.set_sample(
                self.pos,
                channel,
                x * at(inputs[4], i) + old * at(inputs[5], i),
            );
            self.pos += 1;
        }
    }

    fn done(&self) -> DoneAction {
        if self.finished {
            self.done_action
        } else {
            DoneAction::None
        }
    }
}

/// What a [`BufInfo`] UGen reports about the buffer named by its single input
/// (the buffer index). All are block-constant (control-rate-like), mirroring
/// scsynth's `BufSampleRate`, `BufRateScale`, `BufFrames`, `BufChannels` and
/// `BufDur`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufInfoKind {
    /// The file's own sample rate, in Hz.
    SampleRate,
    /// `file_sr / server_sr`: multiply `PlayBuf`'s rate by this so a file at a
    /// different sample rate plays back at its true pitch (the server never
    /// resamples on its own — see the module docs and `docs/schemas.md`).
    RateScale,
    /// Frame count.
    Frames,
    /// Channel count.
    Channels,
    /// Duration in seconds (`frames / file_sr`).
    Duration,
}

/// Reports a static property of a buffer (see [`BufInfoKind`]). Input 0 is the
/// buffer index; the output is constant over the block. An empty/missing slot
/// reports `0`.
pub struct BufInfo(pub BufInfoKind);

impl UGen for BufInfo {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let value = match ctx.buffers.get(index).and_then(|b| b.as_deref()) {
            Some(buf) => {
                let file_sr = buf.sample_rate() as f32;
                match self.0 {
                    BufInfoKind::SampleRate => file_sr,
                    // The engine's rate as a *fact*, not as a time base: this
                    // ratio corrects a file's pitch against the hardware rate,
                    // so it divides by `full_sample_rate` and reads the same at
                    // either rate. Dividing by the instance's own rate would
                    // make a control-rate `BufRateScale` report the block size
                    // (`sr / (sr / 64)`), which silently ruins a `PlayBuf`.
                    BufInfoKind::RateScale => {
                        if ctx.full_sample_rate > 0.0 {
                            file_sr / ctx.full_sample_rate
                        } else {
                            0.0
                        }
                    }
                    BufInfoKind::Frames => buf.frames() as f32,
                    BufInfoKind::Channels => buf.channels() as f32,
                    BufInfoKind::Duration => {
                        if file_sr > 0.0 {
                            buf.frames() as f32 / file_sr
                        } else {
                            0.0
                        }
                    }
                }
            }
            None => 0.0,
        };
        output.fill(value);
    }
}
