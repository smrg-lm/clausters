//! A synth's own buffers, scsynth's `LocalBuf` from its `DelayUGens.cpp`,
//! and the two UGens that fill one at the synth's start: `SetBuf` and
//! `ClearBuf`, which act on any buffer.
//!
//! **A local buffer is the synth's**: allocated with it, at build, sized by
//! constants, and freed with it. Its number is past the pool's last
//! (`NumBuffers` plus its place among the synth's), scsynth's numbering, so
//! every UGen that reads or writes a buffer by number reaches it the same way
//! ([`ProcessCtx::buffer`]). The compiler counts them, so scsynth's
//! `MaxLocalBufs`, which declares the count beforehand, is accepted and does
//! nothing.
//!
//! **`SetBuf` and `ClearBuf` act once, at the synth's start**, as scsynth's
//! constructors do: they are `ir` UGens, and every `ir` UGen runs before the
//! rest on the first block, so what they write is there for every reader in
//! the def wherever it was placed.

use crate::dsp::{ProcessCtx, UGen};

/// `LocalBuf(numChannels, numFrames)`: the number of a buffer of the synth's
/// own, zeroed at the start of each note (scsynth's holds whatever its
/// memory held until something writes it).
pub struct LocalBuf {
    slot: usize,
}

impl LocalBuf {
    /// The buffer the compiler gave this one, in def order.
    pub fn new(slot: Option<usize>) -> Self {
        Self {
            slot: slot.unwrap_or(0),
        }
    }
}

impl UGen for LocalBuf {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill((ctx.buffers.len() + self.slot) as f32);
    }
}

/// `MaxLocalBufs(count)`: scsynth's declaration of how many `LocalBuf`s a
/// def holds, which the compiler here counts itself. Its output is 0.
pub struct MaxLocalBufs;

impl UGen for MaxLocalBufs {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }
}

/// `SetBuf(buf, offset, count, values...)`: writes the values into the
/// buffer from sample `offset` on (its samples interleaved, as scsynth's
/// are), as many as there are and fit. Its output is 0.
pub struct SetBuf;

impl UGen for SetBuf {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
        if inputs[0][0] < 0.0 {
            return;
        }
        let Some(buf) = ctx.buffer(inputs[0][0] as usize) else {
            return;
        };
        let offset = inputs[1][0].max(0.0) as usize;
        // `count` is scsynth's: how many of the inputs after it are values.
        let values = &inputs[3..];
        let count = (inputs[2][0].max(0.0) as usize).min(values.len());
        for (k, v) in values[..count].iter().enumerate() {
            let i = offset + k;
            if i >= buf.len() {
                break;
            }
            buf.set_at(i, v[0]);
        }
    }
}

/// `ClearBuf(buf)`: zeroes the buffer. Its output is 0.
pub struct ClearBuf;

impl UGen for ClearBuf {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
        if inputs[0][0] < 0.0 {
            return;
        }
        if let Some(buf) = ctx.buffer(inputs[0][0] as usize) {
            for i in 0..buf.len() {
                buf.set_at(i, 0.0);
            }
        }
    }
}
