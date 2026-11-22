//! The rate converters, scsynth's from its `LFUGens.cpp`: `K2A` and `A2K`
//! between control and audio rate, `T2A` and `T2K` for triggers, and `DC`,
//! a constant at either rate.

use crate::dsp::{ProcessCtx, Ramp, UGen};

/// `K2A(in)`: a control-rate signal at audio rate, a straight line from the
/// last block's value to this one's, as scsynth's is. An audio-rate input
/// passes through.
#[derive(Default)]
pub struct K2A {
    level: Ramp,
}

impl UGen for K2A {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let x = inputs[0];
        if x.len() > 1 {
            output.copy_from_slice(&x[..output.len()]);
            return;
        }
        let (start, step) = self.level.toward(x[0] as f64, output.len());
        for (i, s) in output.iter_mut().enumerate() {
            *s = (start + step * i as f64) as f32;
        }
    }
}

/// `A2K(in)`: an audio-rate signal at control rate, its block's first sample.
pub struct A2K;

impl UGen for A2K {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(inputs[0][0]);
    }
}

/// `T2K(in)`: an audio-rate trigger at control rate, the block's largest
/// sample (0 when none is positive), so a trigger anywhere in the block is
/// kept.
pub struct T2K;

impl UGen for T2K {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let peak = inputs[0]
            .iter()
            .fold(0.0f32, |m, &x| if x > m { x } else { m });
        output.fill(peak);
    }
}

/// `T2A(in, offset)`: a control-rate trigger at audio rate. At each rising
/// edge of `in`, one sample at `offset` into the block (clipped to it) takes
/// the trigger's value; every other sample is 0.
#[derive(Default)]
pub struct T2A {
    prev: f32,
}

impl UGen for T2A {
    fn reset(&mut self) {
        self.prev = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let level = inputs[0][0];
        output.fill(0.0);
        if self.prev <= 0.0 && level > 0.0 {
            let last = output.len() as i64 - 1;
            let at = (inputs[1][0] as i64).clamp(0, last.max(0)) as usize;
            output[at] = level;
        }
        self.prev = level;
    }
}

/// `DC(in)`: the constant `in`, taken at the synth's start and held, as
/// scsynth's is -- a constant, not a follower.
#[derive(Default)]
pub struct Dc {
    value: Option<f32>,
}

impl UGen for Dc {
    fn reset(&mut self) {
        self.value = None;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let v = *self.value.get_or_insert(inputs[0][0]);
        output.fill(v);
    }
}
