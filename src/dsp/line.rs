//! The one-segment ramps, `Line` and `XLine`.
//!
//! A ramp from `start` to `end` over `dur` seconds, then held at `end`, with a
//! `doneAction` reported when it lands. They are scsynth's `Line`/`XLine`: the
//! per-sample step is worked out **once**, and the inner loop is one addition
//! (or one multiplication) plus a counter. That is the whole reason they live
//! here and not inside [`crate::dsp::envgen`] — the segment engine reads a
//! thirteen-input layout and re-evaluates a shape function every sample, which
//! is the right shape for a breakpoint envelope and far too much machinery for
//! a straight line at audio rate.
//!
//! Being their own implementation, they read their inputs **once**, on the
//! first sample: like scsynth's, these ramps are not modulatable, and a def
//! that changes `end` or `dur` mid-flight changes nothing.

use crate::dsp::{DoneAction, ProcessCtx, UGen, at};

/// Below this magnitude an endpoint counts as zero for the exponential ramp.
const EXP_EPSILON: f64 = 1e-5;

/// Which of the two ramps a [`Line`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineShape {
    /// `Line`: equal steps, `start + t·(end − start)`.
    Linear,
    /// `XLine`: equal *ratios*, `start·(end/start)^t` — the one that sounds
    /// like a straight line when what it drives is a frequency or a gain.
    Exponential,
}

/// How the running level advances, resolved once from the shape and the
/// endpoints. `XLine` degrades to [`Add`](Advance::Add) when its endpoints
/// straddle zero, where a ratio does not exist.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Advance {
    Add,
    Multiply,
}

/// `Line` and `XLine`: a single ramp from `start` to `end` over `dur` seconds,
/// then held, with the full `doneAction` set. Inputs 0 `start`, 1 `end`,
/// 2 `dur`, 3 `done_action`.
///
/// The level accumulates in `f64` from a step computed once, which is what
/// keeps the audio-rate cost at one arithmetic operation per sample. `f64` is
/// load-bearing rather than incidental: an `f32` accumulator over a ten-second
/// ramp drifts far enough to be seen, while this one stays within an ulp of
/// the closed form and the landing is committed exactly anyway — when the
/// counter runs out the level is *assigned* `end`, never merely approached.
///
/// An exponential ramp through or to zero is undefined; a zero endpoint is
/// nudged to a tiny same-signed value and a sign change falls back to a linear
/// step, so `XLine(0, 1, …)` is a very steep rise rather than a `NaN`.
pub struct Line {
    shape: LineShape,
    /// Inputs are read on the first sample and never again; this says whether
    /// that has happened.
    started: bool,
    finished: bool,
    /// The running level, and the step that advances it — added or multiplied
    /// according to `advance`.
    level: f64,
    step: f64,
    advance: Advance,
    /// Samples of ramp left. At zero the level sits on `end` and the ramp is
    /// done from the *next* sample on (see the note in `process`).
    counter: i64,
    end: f64,
    done_action: DoneAction,
}

impl Line {
    pub fn new(shape: LineShape) -> Self {
        Self {
            shape,
            started: false,
            finished: false,
            level: 0.0,
            step: 0.0,
            advance: Advance::Add,
            counter: 0,
            end: 0.0,
            done_action: DoneAction::None,
        }
    }

    /// Reads `start`, `end` and `dur` once and derives the step. `dur` is
    /// floored at one sample, so a zero or negative duration is a single
    /// sample at `start` followed by the hold, never a division by zero.
    fn begin(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]]) {
        let start = at(inputs[0], 0) as f64;
        let end = at(inputs[1], 0) as f64;

        // Seconds to samples in `f32`, the rate's own precision. Widening the
        // duration to `f64` first would be *worse*: `dur` arrives as an `f32`
        // that is already a rounding of the seconds the def meant, and the
        // extra precision preserves that rounding instead of absorbing it, so
        // a ramp asked for exactly 64 samples truncates to 63.
        let samples = ((at(inputs[2], 0) * ctx.sample_rate) as i64).max(1);
        self.counter = samples;
        self.end = end;
        self.level = start;

        let ratio_exists = self.shape == LineShape::Exponential;
        let (a, b) = if ratio_exists {
            // Nudge a zero endpoint to the smallest same-signed level the ratio
            // can be taken of. `copysign` on a zero keeps its sign, so a ramp
            // from -0.0 still goes the way its target says.
            let nudge = |v: f64| {
                if v.abs() < EXP_EPSILON {
                    EXP_EPSILON.copysign(v)
                } else {
                    v
                }
            };
            (nudge(start), nudge(end))
        } else {
            (start, end)
        };

        if ratio_exists && a.signum() == b.signum() {
            self.advance = Advance::Multiply;
            self.level = a;
            self.step = (b / a).powf(1.0 / samples as f64);
        } else {
            self.advance = Advance::Add;
            self.step = (b - a) / samples as f64;
        }
    }
}

impl UGen for Line {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if inputs.len() < 4 {
            output.fill(0.0);
            return;
        }
        // The done action is the one input still read every block: it says what
        // happens to the *node*, and a def is free to re-aim that even though
        // the ramp's own geometry is fixed.
        self.done_action = DoneAction::from_i32(at(inputs[3], 0) as i32);

        if !self.started {
            self.started = true;
            self.begin(ctx, inputs);
        }

        for out in output.iter_mut() {
            if self.counter <= 0 {
                // Landed. The flag is raised here rather than on the sample
                // that exhausted the counter, so "done" means the output is
                // *showing* `end` — which is what `Done` and `FreeSelfWhenDone`
                // watching this ramp report, and when the done action fires.
                self.finished = true;
                *out = self.end as f32;
                continue;
            }
            *out = self.level as f32;
            match self.advance {
                Advance::Add => self.level += self.step,
                Advance::Multiply => self.level *= self.step,
            }
            self.counter -= 1;
            if self.counter == 0 {
                // Arrive exactly, rather than wherever the accumulation got to.
                self.level = self.end;
            }
        }
    }

    fn done(&self) -> DoneAction {
        if self.finished {
            self.done_action
        } else {
            DoneAction::None
        }
    }

    fn is_done(&self) -> bool {
        self.finished
    }
}
