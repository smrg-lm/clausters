//! Bus I/O UGens. This is how synths produce output: a def with no `Out` is
//! silent. scsynth semantics: `Out` **sums** into the bus (several synths on
//! the same bus mix), `ReplaceOut` overwrites, `In` copies from an audio bus,
//! `InCtl` reads a control bus as a constant for the whole block.

use crate::dsp::{NUM_AUDIO_BUSES, NUM_CONTROL_BUSES, ProcessCtx, UGen, at};

/// Bus index inputs are signals like everything else; read once per block.
fn audio_bus(input: &[f32]) -> usize {
    (input[0].max(0.0) as usize).min(NUM_AUDIO_BUSES - 1)
}

/// Input 0: audio bus index. Output: the bus contents.
pub struct In;

impl UGen for In {
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
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let bus = audio_bus(inputs[0]);
        let signal = inputs[1];
        // SAFETY: the M13 stage scheduler never runs two nodes touching the
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
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let bus = audio_bus(inputs[0]);
        let signal = inputs[1];
        // SAFETY: same disjointness argument as `Out`.
        let dest = &mut unsafe { ctx.buses.audio_mut(bus) }[ctx.offset..];
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
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let idx = (inputs[0][0].max(0.0) as usize).min(NUM_CONTROL_BUSES - 1);
        output.fill(ctx.buses.control.get(idx));
    }
}
