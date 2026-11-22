//! The chaotic maps, scsynth's from its `NoiseUGens.cpp` and
//! `ChaosUGens.cpp`: iterated functions sampled at a rate, whose output is
//! deterministic from its initial state and sensitive to it.

use crate::dsp::{ProcessCtx, UGen, at};

/// `Logistic(chaos_param, freq, init)`: the logistic map `y = r y (1 - y)`,
/// iterated `freq` times a second and held between. `r` near 3.57 and above
/// is chaos; below it the map settles into cycles. The first output is the
/// first iterate of `init`.
///
/// scsynth's constructor iterates once before the first block, so its
/// output starts an iterate later.
pub struct Logistic {
    y: f64,
    counter: i64,
    started: bool,
}

impl Logistic {
    pub fn new() -> Self {
        Self {
            y: 0.0,
            counter: 0,
            started: false,
        }
    }
}

impl Default for Logistic {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Logistic {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.started {
            self.started = true;
            self.y = at(inputs[2], 0) as f64;
        }
        let r = at(inputs[0], 0) as f64;
        let freq = at(inputs[1], 0);
        for s in output.iter_mut() {
            if self.counter <= 0 {
                self.counter = ((ctx.sample_rate as f64 / freq.max(0.001) as f64) as i64).max(1);
                self.y = r * self.y * (1.0 - self.y);
            }
            self.counter -= 1;
            *s = self.y as f32;
        }
    }
}
