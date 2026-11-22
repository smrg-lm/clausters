//! A buffer read as a table of values, scsynth's from its `OscUGens.cpp`:
//! `Index` and its interpolating, wrapping and folding forms, the inverses
//! `IndexInBetween` and `DetectIndex`, and `DegreeToKey`, a scale.
//!
//! Input 0 is the buffer, read once a block; the table is its samples in
//! order, channels interleaved, as scsynth reads it. A missing or empty
//! buffer outputs 0.

use crate::dsp::buffer::Buffer;
use crate::dsp::{ProcessCtx, UGen, at};

/// The table a UGen's input 0 names, if it has values.
fn table<'a>(ctx: &'a ProcessCtx, inputs: &[&[f32]]) -> Option<&'a Buffer> {
    let index = inputs[0][0];
    if index < 0.0 {
        return None;
    }
    let buf = ctx.buffers.get(index as usize)?.as_deref()?;
    (buf.frames() * buf.channels() > 0).then_some(buf)
}

/// Value `i` of the table, its samples in order.
#[inline]
fn value(buf: &Buffer, i: usize) -> f32 {
    let ch = buf.channels();
    buf.sample(i / ch, i % ch)
}

/// The number of values in the table.
#[inline]
fn len(buf: &Buffer) -> usize {
    buf.frames() * buf.channels()
}

/// How an index outside the table is brought into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexMode {
    /// `Index`: clipped to the ends.
    Clip,
    /// `WrapIndex`: wrapped around.
    Wrap,
    /// `FoldIndex`: reflected at the ends.
    Fold,
}

/// `Index`, `WrapIndex`, `FoldIndex`: the table's value at the truncated
/// index (input 1), the index clipped, wrapped or folded into the table.
pub struct Index {
    mode: IndexMode,
}

impl Index {
    pub fn new(mode: IndexMode) -> Self {
        Self { mode }
    }
}

impl UGen for Index {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let Some(buf) = table(ctx, inputs) else {
            output.fill(0.0);
            return;
        };
        let n = len(buf) as i64;
        let max = n - 1;
        for (i, s) in output.iter_mut().enumerate() {
            let k = at(inputs[1], i) as i64;
            let k = match self.mode {
                IndexMode::Clip => k.clamp(0, max),
                IndexMode::Wrap => k.rem_euclid(n),
                IndexMode::Fold => {
                    if max == 0 {
                        0
                    } else {
                        let c = k.rem_euclid(2 * max);
                        if c > max { 2 * max - c } else { c }
                    }
                }
            };
            *s = value(buf, k as usize);
        }
    }
}

/// `IndexL`: the table read at a fractional index (input 1), linearly
/// between the two values either side, the index clipped to the table.
///
/// scsynth truncates a negative index toward zero and still interpolates by
/// its fraction, so between -1 and 0 it reads between the first two values;
/// here an index below the table reads its first value, as one past it reads
/// its last.
pub struct IndexL;

impl UGen for IndexL {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let Some(buf) = table(ctx, inputs) else {
            output.fill(0.0);
            return;
        };
        let max = len(buf) - 1;
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[1], i).clamp(0.0, max as f32);
            let k = x as usize;
            let frac = x - k as f32;
            let a = value(buf, k);
            let b = value(buf, (k + 1).min(max));
            *s = a + frac * (b - a);
        }
    }
}

/// `IndexInBetween`: the inverse of `IndexL` over an ascending table -- the
/// fractional index at which the table, read linearly, reaches the input
/// (input 1). Below the first value it is 0, at or past the last the last
/// index. The search is scsynth's, the first value above the input from the
/// start, which is what makes an unsorted table answer as scsynth's does.
pub struct IndexInBetween;

impl UGen for IndexInBetween {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let Some(buf) = table(ctx, inputs) else {
            output.fill(0.0);
            return;
        };
        let n = len(buf);
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[1], i);
            *s = (n - 1) as f32;
            for k in 0..n {
                let v = value(buf, k);
                if v > x {
                    *s = if k == 0 {
                        0.0
                    } else {
                        let prev = value(buf, k - 1);
                        (x - prev) / (v - prev) + (k - 1) as f32
                    };
                    break;
                }
            }
        }
    }
}

/// `DetectIndex`: the index of the first table value equal to the input
/// (input 1), or -1 where there is none. An input that holds still is not
/// searched again.
#[derive(Default)]
pub struct DetectIndex {
    last: Option<(f32, f32)>,
}

impl UGen for DetectIndex {
    fn reset(&mut self) {
        self.last = None;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let Some(buf) = table(ctx, inputs) else {
            output.fill(0.0);
            return;
        };
        let n = len(buf);
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[1], i);
            let found = match self.last {
                Some((input, index)) if input == x => index,
                _ => {
                    let index = (0..n)
                        .find(|&k| value(buf, k) == x)
                        .map_or(-1.0, |k| k as f32);
                    self.last = Some((x, index));
                    index
                }
            };
            *s = found;
        }
    }
}

/// `DegreeToKey(bufnum, in, octave)`: a scale degree to a key. The table is
/// one octave of the scale (its steps above the tonic); degree `in`
/// (floored) is the table's value at `in` modulo its size, plus `octave`
/// times how many whole tables the degree is past the first, below it
/// included -- degree -1 is the last step an octave down.
///
/// scsynth reads past the table for a negative degree that is a whole
/// number of octaves (C's remainder), and truncates `octave` to an integer;
/// here the degree is divided as a scale divides it and the octave is any
/// size (a non-octave scale's period).
pub struct DegreeToKey;

impl UGen for DegreeToKey {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let Some(buf) = table(ctx, inputs) else {
            output.fill(0.0);
            return;
        };
        let n = len(buf) as i64;
        let octave = at(inputs[2], 0);
        for (i, s) in output.iter_mut().enumerate() {
            let degree = at(inputs[1], i).floor() as i64;
            *s = value(buf, degree.rem_euclid(n) as usize) + octave * degree.div_euclid(n) as f32;
        }
    }
}
