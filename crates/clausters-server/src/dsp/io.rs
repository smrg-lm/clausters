//! Bus I/O UGens. This is how synths produce output: a def with no `Out` is
//! silent. scsynth semantics: `Out` **sums** into the bus (several synths on
//! the same bus mix), `ReplaceOut` overwrites, `In` copies from an audio bus,
//! `InCtl` reads a control bus as a constant for the whole block.

use crate::dsp::{ProcessCtx, UGen, at};

/// Bus index inputs are signals like everything else; read once per block.
///
/// Not clamped here: how many buses there are is a **configured** resource, so
/// the only place that knows is the `Buses` this block is running against, and
/// both doors there clamp an out-of-range index to the last bus. Clamping to a
/// compile-time count instead is how a server configured with more buses than
/// the default would have quietly moved a write from bus 500 to bus 127.
fn audio_bus(input: &[f32]) -> usize {
    input[0].max(0.0) as usize
}

/// Input 0: audio bus index. Output: the bus contents.
pub struct In;

impl UGen for In {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let bus = audio_bus(inputs[0]);
        let from = ctx.offset;
        output.copy_from_slice(&ctx.buses.audio(bus)[from..from + output.len()]);
    }
}

/// Inputs: bus index, signal. Sums the signal into the bus; the output wire
/// passes the signal through.
pub struct Out;

impl UGen for Out {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let bus = audio_bus(inputs[0]);
        let signal = inputs[1];
        // SAFETY: the stage scheduler never runs two nodes touching the
        // same bus concurrently (single-threaded otherwise).
        let dest = &mut unsafe { ctx.buses.audio_mut(bus) }[ctx.offset..];
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(signal, i);
            dest[i] += x;
            *s = x;
        }
    }
}

/// Inputs: bus index, signal. Overwrites the bus instead of summing.
pub struct ReplaceOut;

impl UGen for ReplaceOut {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let bus = audio_bus(inputs[0]);
        let signal = inputs[1];
        // SAFETY: same disjointness argument as `Out`.
        let dest = &mut unsafe { ctx.buses.audio_replace(bus) }[ctx.offset..];
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(signal, i);
            dest[i] = x;
            *s = x;
        }
    }
}

/// Input 0: control bus index. Output: the bus value, constant over the block.
pub struct InCtl;

impl UGen for InCtl {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let idx = inputs[0][0].max(0.0) as usize;
        output.fill(ctx.buses.control.get(idx));
    }
}

/// Input 0: control bus index, input 1: signal. Writes the signal's latest
/// value to a **control** bus -- the write-side counterpart of `InCtl`, so a node
/// reading that bus (via `/node_map` or `InCtl`) tracks it. A control bus holds one
/// value per block, so the block's last sample is committed. The signal is also
/// passed through as the output, so it can still feed other UGens.
pub struct OutCtl;

impl UGen for OutCtl {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let idx = at(inputs[0], 0).max(0.0) as usize;
        let signal = inputs[1];
        for (i, s) in output.iter_mut().enumerate() {
            *s = at(signal, i);
        }
        // **The block's value is the one it starts at.** A control bus carries
        // one number a block, and whoever reads it holds that number for the
        // whole block -- so writing the block's *last* sample hands a reader
        // running later in the same block a value a block early. That is
        // inaudible on anything slow and a click on the first block of a fade:
        // an envelope that says zero at a box's start put out the value a
        // block after it instead, and a transport thawed from a stop has nothing
        // to smooth it with.
        ctx.buses.control.set(idx, at(signal, 0));
    }
}

/// `InFeedback(bus)`: an audio bus read whatever the order of the nodes --
/// this block's samples if the bus has been written this block, else the
/// last block's, a block late: the feedback between synths `In` cannot read,
/// since a bus written by a later node is read before it is written.
pub struct InFeedback;

impl UGen for InFeedback {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let bus = audio_bus(inputs[0]);
        let from = ctx.offset;
        output.copy_from_slice(&ctx.buses.audio_feedback(bus)[from..from + output.len()]);
    }
}

/// `InTrig(bus)`: a control bus as a trigger -- its value in the block it was
/// set, 0 in the others. A set is the server's (`/control_set`, an `Out` at
/// control rate) even to the value the bus held, or any write that changes
/// the value, the shared segment's included.
#[derive(Default)]
pub struct InTrig {
    last: Option<(u32, u32)>,
}

impl UGen for InTrig {
    fn reset(&mut self) {
        self.last = None;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let idx = inputs[0][0].max(0.0) as usize;
        let value = ctx.buses.control.get(idx);
        let now = (ctx.buses.control.stamp(idx), value.to_bits());
        let set = self.last.is_some_and(|last| last != now);
        self.last = Some(now);
        output.fill(if set { value } else { 0.0 });
    }
}

/// `LagIn(bus, lag)`: a control bus through a one-pole lag of `lag` seconds
/// to -60 dB, starting at the bus's value.
#[derive(Default)]
pub struct LagIn {
    y: Option<f32>,
}

impl UGen for LagIn {
    fn reset(&mut self) {
        self.y = None;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let idx = inputs[0][0].max(0.0) as usize;
        let x = ctx.buses.control.get(idx);
        let lag = at(inputs[1], 0);
        let b1 = if lag == 0.0 {
            0.0
        } else {
            (0.001f64.ln() / (lag as f64 * ctx.sample_rate as f64)).exp() as f32
        };
        let mut y = *self.y.get_or_insert(x);
        for s in output.iter_mut() {
            y = x + b1 * (y - x);
            *s = y;
        }
        self.y = Some(y);
    }
}

/// `XOut(bus, xfade, signal)`: the signal crossfaded into the bus,
/// `bus + xfade (signal - bus)` -- 1 replaces it, 0 leaves it. A
/// control-rate `xfade` is a line across the block, as scsynth's is.
#[derive(Default)]
pub struct XOut {
    xfade: crate::dsp::Ramp,
}

impl UGen for XOut {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let bus = audio_bus(inputs[0]);
        let (xf, signal) = (inputs[1], inputs[2]);
        let (mut k, step) = if xf.len() == 1 {
            self.xfade.toward(xf[0] as f64, output.len())
        } else {
            (0.0, 0.0)
        };
        let from = ctx.offset;
        let mut was = [0.0f32; crate::dsp::BLOCK_SIZE];
        was[..output.len()].copy_from_slice(&ctx.buses.audio(bus)[from..from + output.len()]);
        // SAFETY: same disjointness argument as `ReplaceOut`.
        let dest = &mut unsafe { ctx.buses.audio_replace(bus) }[from..];
        for (i, s) in output.iter_mut().enumerate() {
            let x = if xf.len() == 1 { k as f32 } else { xf[i] };
            let v = was[i] + x * (at(signal, i) - was[i]);
            dest[i] = v;
            *s = at(signal, i);
            k += step;
        }
    }
}
