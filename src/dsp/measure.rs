//! The meter: what a level looks like when a person has to read it.
//!
//! A picture of the raw block peak is unreadable — it flickers, and a transient
//! shows for one frame of the screen or for none at all. Every meter anybody has
//! ever read answers three rules instead: the attack is instantaneous, the fall
//! is a fixed number of decibels per second, and a peak is held long enough to
//! be seen. Those rules are arithmetic, so they live in
//! [`clausters_core::measure::Ballistics`] and this is the UGen that runs them:
//! two clients drawing two different falls off one signal would be two answers
//! to a question that has one.
//!
//! It emits the value rather than writing it anywhere, so what a caller does
//! with it — a control bus per channel in a mixer, a `SendReply`, a signal that
//! drives something — stays the caller's.

use clausters_core::measure::Ballistics;

use crate::dsp::{ProcessCtx, UGen, at};

/// A meter's ballistics over a signal. Inputs: 0 signal, 1 fall in decibels per
/// second, 2 the seconds a peak is held before it begins to fall.
///
/// One block is one measurement: the block's own peak goes in, and the whole
/// block comes out at what the meter now reads. That is what a meter *is* — a
/// number per block, which is also exactly what a control bus carries — and it
/// is why a finer answer would be samples nothing can read.
pub struct Meter {
    state: Ballistics,
}

impl Meter {
    pub fn new() -> Self {
        Self {
            state: Ballistics::new(),
        }
    }
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Meter {
    fn resume(&mut self) {
        // A held peak is "the loudest thing lately", and lately ended when the
        // transport did: a meter thawed with the old pass's peak still up
        // reports a level the piece has not played yet.
        self.state = Ballistics::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let signal = inputs[0];
        let mut peak = 0.0f32;
        for i in 0..output.len() {
            peak = peak.max(at(signal, i).abs());
        }
        let seconds = if ctx.sample_rate > 0.0 {
            output.len() as f32 / ctx.sample_rate
        } else {
            0.0
        };
        let level = self
            .state
            .tick(peak, seconds, at(inputs[1], 0), at(inputs[2], 0));
        output.fill(level);
    }
}
