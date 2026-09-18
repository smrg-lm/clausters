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
        // reports a level the transport has not played yet.
        self.state = Ballistics::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let signal = inputs[0];
        // **Every sample the input carries**, not every sample this UGen
        // emits. A `kr` meter emits one number a block and its audio-rate
        // input is still the whole block, so walking the *output* would read
        // one sample in sixty-four and call it the peak — which is exactly the
        // reading a meter exists to not give.
        let mut peak = 0.0f32;
        for &sample in signal {
            peak = peak.max(sample.abs());
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

/// **Counting overs**: how many times the signal was flattened, since the pass
/// began. Inputs: 0 signal, 1 the ceiling in linear amplitude, 2 how many
/// consecutive samples at or over it count as one over.
///
/// The count and not the flag, for the reason a level is a level and not a
/// colour: the flag is a *reader's* state — it stays lit until a hand puts it
/// out, and two windows watching one bus each have their own — while the count
/// is the signal's, and it is what a reader differences to notice an over it
/// has not seen yet. [`clausters_core::measure::ClipLatch`] is the other half,
/// and it lives wherever the mark is drawn.
///
/// It runs per **sample** rather than per block, unlike [`Meter`]: a run is the
/// whole of what distinguishes a flattened peak from a sample that legitimately
/// reached full scale, and a block peak has already lost it.
pub struct ClipCount {
    state: clausters_core::measure::ClipCount,
}

impl ClipCount {
    pub fn new() -> Self {
        Self {
            state: clausters_core::measure::ClipCount::new(),
        }
    }
}

impl Default for ClipCount {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for ClipCount {
    fn resume(&mut self) {
        // A new pass counts its own overs, and a count that goes backwards is
        // what puts a reader's mark out without anybody reaching for it.
        self.state.reset();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let signal = inputs[0];
        let ceiling = at(inputs[1], 0);
        let run = at(inputs[2], 0).max(1.0) as u32;
        // The input's samples, not this UGen's: see [`Meter::process`]. A run
        // is the whole of what this counts, so reading one sample a block
        // would not merely be coarse — it would be counting a different thing.
        self.state.feed_block(signal, ceiling, run);
        output.fill(self.state.overs() as f32);
    }
}

/// **A meter's level, over the reconstructed signal.** Inputs: 0 signal, 1 fall
/// in decibels per second, 2 the seconds a peak is held before it begins to
/// fall -- the same three [`Meter`] takes, so a script swaps one for the other.
///
/// What goes in is not the block's largest sample but its **true peak**: the
/// signal between the samples, reconstructed with the ITU-R BS.1770-4 Annex 2
/// filter ([`clausters_core::resample::TruePeakMeter`]). A signal whose samples
/// all sit at full scale can reach three decibels over it between them, and
/// every converter sees that; a sample meter cannot. The reading is therefore
/// in **dBTP**, and it is never below what [`Meter`] reads off the same signal.
///
/// The filter's context lives across blocks, so a peak that straddles two of
/// them is still one peak. It walks the **input's** samples, as [`Meter`] does:
/// at control rate the output is one number a block and the input is still the
/// whole block.
pub struct TruePeak {
    filter: clausters_core::resample::TruePeakMeter,
    state: Ballistics,
}

impl TruePeak {
    pub fn new() -> Self {
        Self {
            filter: clausters_core::resample::TruePeakMeter::new(),
            state: Ballistics::new(),
        }
    }
}

impl Default for TruePeak {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for TruePeak {
    fn resume(&mut self) {
        // A new pass measures its own signal: neither the old pass's held peak
        // nor the tail of its last block in the filter's window.
        self.filter.reset();
        self.state = Ballistics::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let peak = self.filter.feed_block(inputs[0]);
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
