//! Segment-based envelope generator, modelled on SuperCollider's `EnvGen`.
//!
//! The envelope is a list of segments; each segment interpolates from the
//! previous level to a target over a duration, following one of the SC shape
//! curves. A `gate` drives it: a rising edge (re)triggers from the initial
//! level; while the gate stays open the envelope **sustains** at the
//! `releaseNode` (holding that level, or, with a `loopNode`, cycling the
//! segments between the two) until the gate falls, then it plays the
//! remaining segments. When the last segment ends the UGen reports its
//! `doneAction` through [`UGen::done`], which the engine turns into freeing the
//! node (see `node::NodeTree` and `server::engine`).
//!
//! Beside it, the two other envelopes of scsynth's library: [`IEnvGen`], the
//! same segments read at a position rather than played in time, and
//! [`Linen`], the trapezoid of a gate.

// The segment interpolation itself (the SC shape curves) lives once in the
// shared core, so a client drawing an envelope evaluates exactly what this
// UGen plays.
use clausters_core::envshape::{Segment, shape_value};

use crate::dsp::{DoneAction, ProcessCtx, UGen, at};

/// Number of leading, non-segment inputs (see the layout comment in `process`).
const HEADER_INPUTS: usize = 9;
/// Inputs per segment: target, duration, shape, curve.
const SEGMENT_INPUTS: usize = 4;

pub struct EnvGen {
    /// The segment playing, or `None` before the first gate.
    stage: Option<i64>,
    /// Samples left in the stage.
    counter: i64,
    /// The stage's length in samples, fractional: the shape is read at
    /// `n / length` while only the whole part is played, and what is left
    /// over carries into the next stage (`residual`), so an envelope keeps
    /// time over many short stages.
    length: f64,
    residual: f64,
    /// Samples into the stage.
    n: i64,
    /// The output level, and the level the stage started from and aims at.
    level: f64,
    end_level: f64,
    segment: Segment,
    /// The stage's shape as played: scsynth turns a one-sample stage and a
    /// curve of no curvature into a line.
    shape: i32,
    sustaining: bool,
    gate_prev: f32,
    /// Whether any sample has been processed yet. The gate's state at birth is
    /// only knowable on the first sample, and a gate *already* closed there
    /// counts as a release edge (see the comment at the edge detection).
    primed: bool,
    /// Set once the gate falls; cleared on a fresh trigger. Distinguishes
    /// "sustaining, waiting for release" from "released, playing out".
    released: bool,
    finished: bool,
    done_action: DoneAction,
}

/// scsynth's shape numbers that change how a stage starts.
const SHAPE_STEP: i32 = 0;
const SHAPE_LINEAR: i32 = 1;
const SHAPE_CURVE: i32 = 5;
const SHAPE_HOLD: i32 = 8;

impl Default for EnvGen {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvGen {
    pub fn new() -> Self {
        Self {
            stage: None,
            counter: 0,
            length: 1.0,
            residual: 0.0,
            n: 0,
            level: 0.0,
            end_level: 0.0,
            segment: Segment::new(SHAPE_HOLD, 0.0, 0.0, 0.0),
            shape: SHAPE_HOLD,
            sustaining: false,
            gate_prev: 0.0,
            primed: false,
            released: false,
            finished: false,
            done_action: DoneAction::None,
        }
    }

    /// Starts stage `stage`, scsynth's `EnvGen_initSegment`: its target is the
    /// scaled level, its length the scaled time (or `dur`, a forced
    /// release's), and it starts from wherever the output is -- except after
    /// a hold, which jumps to the level it held towards.
    fn start_stage(&mut self, inputs: &[&[f32]], stage: i64, dur: Option<f64>, sr: f64) -> bool {
        let base = HEADER_INPUTS + stage as usize * SEGMENT_INPUTS;
        if stage < 0 || base + SEGMENT_INPUTS > inputs.len() {
            return false;
        }
        self.stage = Some(stage);
        let (scale, bias) = (at(inputs[1], 0) as f64, at(inputs[2], 0) as f64);
        let previous_end = self.end_level;
        if self.shape == SHAPE_HOLD {
            self.level = previous_end;
        }
        let end = at(inputs[base], 0) as f64 * scale + bias;
        let dur = dur.unwrap_or(at(inputs[base + 1], 0) as f64 * at(inputs[3], 0) as f64);
        let mut shape = at(inputs[base + 2], 0) as i32;
        let curve = at(inputs[base + 3], 0);
        self.end_level = end;
        let samples = dur * sr + self.residual;
        self.counter = (samples as i64).max(1);
        self.length = samples.max(1.0);
        self.residual = samples - self.counter as f64;
        if self.counter == 1 || (shape == SHAPE_CURVE && curve.abs() < 0.001) {
            shape = SHAPE_LINEAR;
        }
        self.shape = shape;
        let from = if shape == SHAPE_HOLD {
            previous_end
        } else {
            self.level
        };
        self.segment = Segment::new(shape, curve, from as f32, end as f32);
        self.level = match shape {
            SHAPE_STEP => end,
            SHAPE_HOLD => previous_end,
            _ => self.level,
        };
        self.n = 0;
        self.sustaining = false;
        true
    }

    /// The stage after the one that ended, scsynth's `EnvGen_nextSegment`:
    /// the end, a sustain at the release node (or a jump back to the loop
    /// node), or the next stage.
    fn next_stage(&mut self, inputs: &[&[f32]], sr: f64) {
        let stages = at(inputs[6], 0) as i64;
        let release_node = at(inputs[7], 0) as i64;
        let loop_node = at(inputs[8], 0) as i64;
        let Some(stage) = self.stage else {
            self.counter = i64::MAX;
            return;
        };
        if stage + 1 >= stages {
            self.finished = true;
            self.counter = i64::MAX;
        } else if stage + 1 == release_node && !self.released {
            if loop_node >= 0 && loop_node < stages {
                if !self.start_stage(inputs, loop_node, None, sr) {
                    self.finished = true;
                }
            } else {
                self.counter = i64::MAX;
                self.sustaining = true;
                self.level = self.end_level;
            }
        } else if !self.start_stage(inputs, stage + 1, None, sr) {
            self.finished = true;
            self.counter = i64::MAX;
        }
    }

    /// The gate's edges, scsynth's `check_gate`: a rise starts over, a gate at
    /// or below -1 is a forced release taking `-gate - 1` seconds, and a fall
    /// releases into the stage after the release node.
    fn gate(&mut self, inputs: &[&[f32]], gate: f32, sr: f64) {
        // A falling gate releases -- and so does a gate found *already*
        // closed on the very first sample. A live client's note-on and
        // note-off can land in the same command drain (both applied before
        // the node's first block), so the envelope never sees an edge;
        // without this it would wait forever on a closed gate -- a stuck,
        // audible node. Born released, it plays the release stage from the
        // initial level and finishes, so the done action still frees the
        // node.
        let born = !self.primed;
        self.primed = true;
        let release_node = at(inputs[7], 0) as i64;
        if gate > 0.0 && self.gate_prev <= 0.0 {
            self.stage = Some(-1);
            self.released = false;
            self.finished = false;
            self.sustaining = false;
            self.counter = 0;
            self.residual = 0.0;
        } else if gate <= -1.0 && self.gate_prev > -1.0 {
            let dur = (-gate - 1.0) as f64;
            let last = at(inputs[6], 0) as i64 - 1;
            self.released = true;
            self.finished = false;
            self.residual = 0.0;
            self.start_stage(inputs, last, Some(dur), sr);
            self.residual = 0.0;
        } else if gate <= 0.0 && (self.gate_prev > 0.0 || born) && !self.released {
            self.released = true;
            if release_node >= 0 {
                self.stage = Some(release_node - 1);
                self.finished = false;
                self.sustaining = false;
                self.counter = 0;
                self.residual = 0.0;
            } else if born {
                // No release node: a closed gate at birth plays straight
                // through, as an open one that closed at once would.
                self.stage = Some(-1);
                self.counter = 0;
            }
        }
        self.gate_prev = gate;
    }
}

impl UGen for EnvGen {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        // inputs:
        // 0: gate, 1: levelScale, 2: levelBias, 3: timeScale, 4: doneAction
        // 5: initLevel, 6: numSegments, 7: releaseNode, 8: loopNode
        // 9...: [target, duration, shape, curve] per segment
        if inputs.len() < HEADER_INPUTS {
            output.fill(0.0);
            return;
        }
        let sr = ctx.sample_rate as f64;
        self.done_action = DoneAction::from_i32(at(inputs[4], 0) as i32);
        if !self.primed {
            // scsynth's constructor: the output starts at the scaled initial
            // level, or at the first target when the first stage holds.
            self.level =
                at(inputs[5], 0) as f64 * at(inputs[1], 0) as f64 + at(inputs[2], 0) as f64;
            self.end_level = self.level;
            if inputs.len() > HEADER_INPUTS + 2
                && at(inputs[HEADER_INPUTS + 2], 0) as i32 == SHAPE_HOLD
            {
                self.level = at(inputs[HEADER_INPUTS], 0) as f64;
            }
        }
        // A control-rate gate is checked once, at the slice's start; an
        // audio-rate one at every sample.
        let gate = inputs[0];
        for (i, out) in output.iter_mut().enumerate() {
            if i == 0 || gate.len() > 1 {
                self.gate(inputs, at(gate, i), sr);
            }
            if self.counter <= 0 && !self.finished {
                self.next_stage(inputs, sr);
            }
            *out = self.level as f32;
            if self.finished || self.sustaining || self.stage.is_none() {
                self.level = if self.finished {
                    self.end_level
                } else {
                    self.level
                };
                continue;
            }
            self.n += 1;
            self.counter -= 1;
            match self.shape {
                SHAPE_STEP | SHAPE_HOLD => {}
                _ => {
                    let t = (self.n as f64 / self.length).min(1.0);
                    self.level = self.segment.at(t as f32) as f64;
                }
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

/// **One envelope segment, read at a position**: `phase` in `[0, 1]`
/// (clamped) through the segment from level `from` to level `to`, with the
/// shape number and curvature an `EnvGen` segment takes -- evaluated by
/// `clausters_core::envshape::shape_value`, the function `EnvGen` plays and a
/// view draws.
///
/// `EnvGen` runs on time: it starts on a gate and moves at its own pace. This
/// is the same arithmetic driven from outside, which is what a segment that
/// must follow a **position** needs -- a box's fade, read off the transport,
/// which a locate jumps and a stop holds.
pub struct EnvShape;

impl UGen for EnvShape {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, s) in output.iter_mut().enumerate() {
            let phase = at(inputs[0], i).clamp(0.0, 1.0);
            let (from, to) = (at(inputs[1], i), at(inputs[2], i));
            let shape = at(inputs[3], i) as i32;
            let curve = at(inputs[4], i);
            *s = shape_value(shape, curve, from, to, phase);
        }
    }
}

/// Leading inputs of an `IEnvGen` before its segments: `index`, `offset`,
/// the initial level, the segment count and the total duration.
const IENV_HEADER: usize = 5;

/// scsynth's `IEnvGen(index, offset, level0, segments, total, ...)`: an
/// envelope **read at a position** -- `index`, in the envelope's time units
/// from `offset` -- rather than played in time, so anything can drive it
/// (a phasor, a fader, another signal) and it can run backwards or jump.
/// Each segment is `duration, shape, curve, level`, scsynth's layout for it
/// (`Env.asArrayForInterpolation`), and its shape is computed by the function
/// `EnvGen` plays and a view draws.
///
/// Before the start the output is the initial level, past `total` the last
/// level. Two of scsynth's are not copied: it truncates a segment's
/// curvature to an integer, and its `hold` shape answers the last value it
/// computed, which depends on where the index was before; here the curvature
/// is the one given and a held segment is its start level, a function of the
/// position alone. The levels are read live, where scsynth reads them once.
pub struct IEnvGen {
    /// The last position read and the level it gave: an index standing still
    /// costs nothing.
    last: Option<(f32, f32)>,
}

impl IEnvGen {
    pub fn new() -> Self {
        Self { last: None }
    }

    fn level(inputs: &[&[f32]], point: f32, i: usize) -> f32 {
        let value = |k: usize| inputs.get(k).map_or(0.0, |x| at(x, i));
        let init = value(2);
        let stages =
            (value(3).max(0.0) as usize).min((inputs.len() - IENV_HEADER) / SEGMENT_INPUTS);
        let total = value(4);
        if point >= total {
            return value(IENV_HEADER - 1 + stages * SEGMENT_INPUTS);
        }
        if point <= 0.0 {
            return init;
        }
        let mut start = 0.0f32;
        let mut begin = init;
        for j in 0..stages {
            let base = IENV_HEADER + j * SEGMENT_INPUTS;
            let dur = value(base);
            let end = value(base + 3);
            if point < start + dur || j + 1 == stages {
                let pos = if dur > 0.0 {
                    (point - start) / dur
                } else {
                    1.0
                };
                let shape = value(base + 1) as i32;
                return Segment::new(shape, value(base + 2), begin, end).at(pos.min(1.0));
            }
            start += dur;
            begin = end;
        }
        init
    }
}

impl Default for IEnvGen {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for IEnvGen {
    fn reset(&mut self) {
        self.last = None;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let live = inputs[2..].iter().any(|x| x.len() > 1);
        for (i, s) in output.iter_mut().enumerate() {
            let index = at(inputs[0], i);
            *s = match self.last {
                Some((at_index, level)) if at_index == index && !live => level,
                _ => {
                    let point = (index - at(inputs[1], i)).max(0.0);
                    let level = Self::level(inputs, point, i);
                    self.last = Some((index, level));
                    level
                }
            };
        }
    }
}

/// scsynth's `Linen(gate, attack, sustain, release, done_action)`: the
/// trapezoid of a gate -- a straight rise to `sustain` over `attack` seconds
/// when the gate opens, held while it stays open, a straight fall to 0 over
/// `release` when it closes, and then `done_action`. A gate opening again
/// rises from wherever the level is; a gate at or below -1 releases at once
/// over `-gate - 1` seconds, scsynth's forced release. A gate closing during
/// the rise releases from there, where scsynth's finishes the rise first.
///
/// A control-rate UGen, as scsynth's is: its times count control periods.
pub struct Linen {
    stage: u8,
    level: f32,
    slope: f32,
    counter: i64,
    prev_gate: f32,
    started: bool,
    done_action: DoneAction,
    finished: bool,
}

impl Linen {
    pub fn new() -> Self {
        Self {
            stage: 4,
            level: 0.0,
            slope: 0.0,
            counter: 0,
            prev_gate: 0.0,
            started: false,
            done_action: DoneAction::None,
            finished: false,
        }
    }

    fn segment(&mut self, to: f32, time: f32, sr: f32) {
        // In double, as scsynth multiplies: a time that is a whole number of
        // periods in decimal is a hair under it in binary, and truncates.
        let counter = ((time as f64 * sr as f64) as i64).max(1);
        self.slope = (to - self.level) / counter as f32;
        self.counter = counter;
    }
}

impl Default for Linen {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Linen {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate;
        let gate = at(inputs[0], 0);
        self.done_action = DoneAction::from_i32(at(inputs[4], 0) as i32);
        if !self.started {
            self.started = true;
            if gate <= -1.0 {
                // Released before it began: it sustains at 0, and the
                // release below runs from there.
                self.stage = 1;
            }
        }
        if self.prev_gate <= 0.0 && gate > 0.0 {
            self.finished = false;
            self.stage = 0;
            self.segment(at(inputs[2], 0), at(inputs[1], 0), sr);
        }
        if self.stage == 0 && gate <= 0.0 {
            // A gate closing during the rise releases from where the level
            // is, as an envelope's release node does; scsynth's finishes
            // the rise first, holding a short gate open for the whole attack.
            self.stage = 1;
        }
        let out = match self.stage {
            0 | 2 => {
                let out = self.level;
                self.level += self.slope;
                self.counter -= 1;
                if self.counter == 0 {
                    self.stage += 1;
                }
                out
            }
            1 => {
                let out = self.level;
                if gate <= -1.0 {
                    self.stage = 2;
                    self.segment(0.0, -gate - 1.0, sr);
                } else if gate <= 0.0 {
                    self.stage = 2;
                    self.segment(0.0, at(inputs[3], 0), sr);
                }
                out
            }
            3 => {
                self.finished = true;
                self.stage = 4;
                0.0
            }
            _ => 0.0,
        };
        output.fill(out);
        self.prev_gate = gate;
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
