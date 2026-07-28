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

// The segment interpolation itself (the SC shape curves) lives once in the
// shared core, so a client drawing an envelope evaluates exactly what this
// UGen plays.
use clausters_core::envshape::shape_value;

use crate::dsp::{DoneAction, ProcessCtx, UGen, at};

/// Number of leading, non-segment inputs (see the layout comment in `process`).
const HEADER_INPUTS: usize = 9;
/// Inputs per segment: target, duration, shape, curve.
const SEGMENT_INPUTS: usize = 4;

pub struct EnvGen {
    current_segment: usize,
    segment_phase: usize,
    gate_prev: f32,
    /// Whether any sample has been processed yet. The gate's state at birth is
    /// only knowable on the first sample, and a gate *already* closed there
    /// counts as a release edge (see the comment at the edge detection).
    primed: bool,
    /// Set once the gate falls; cleared on a fresh trigger. Distinguishes
    /// "sustaining, waiting for release" from "released, playing out".
    released: bool,
    finished: bool,
    start_level: f32,
    last_val: f32,
    done_action: DoneAction,
}

impl Default for EnvGen {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvGen {
    pub fn new() -> Self {
        Self {
            current_segment: 0,
            segment_phase: 0,
            gate_prev: 0.0,
            primed: false,
            released: false,
            finished: false,
            start_level: 0.0,
            last_val: 0.0,
            done_action: DoneAction::None,
        }
    }
}

impl UGen for EnvGen {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        // inputs:
        // 0: gate, 1: levelScale, 2: levelBias, 3: timeScale, 4: doneAction
        // 5: initLevel, 6: numSegments, 7: releaseNode, 8: loopNode
        // 9...: [target, duration, shape, curve] per segment
        if inputs.len() < HEADER_INPUTS {
            output.fill(0.0);
            return;
        }

        self.done_action = DoneAction::from_i32(at(inputs[4], 0) as i32);

        let num_segments = at(inputs[6], 0) as usize;
        // Index into the levels array (== the segment index to resume from) at
        // which the envelope sustains while the gate is open; < 0 disables the
        // sustain, so the envelope plays straight through.
        let release_node = at(inputs[7], 0) as i32;
        let has_release = release_node >= 0 && (release_node as usize) < num_segments;
        let release_node = release_node as usize;
        // Sustain loop: while the gate is held, on reaching the release node
        // jump back to the loop node instead of holding, replaying the segments
        // in `[loopNode, releaseNode)` in a cycle. The loop node must sit before
        // the release node so the cycle makes progress; otherwise it is ignored.
        let loop_node = at(inputs[8], 0) as i32;
        let has_loop = has_release && loop_node >= 0 && (loop_node as usize) < release_node;
        let loop_node = loop_node.max(0) as usize;

        for (i, out) in output.iter_mut().enumerate() {
            let gate = at(inputs[0], i);
            let level_scale = at(inputs[1], i);
            let level_bias = at(inputs[2], i);
            let time_scale = at(inputs[3], i);

            let trig = gate > 0.0 && self.gate_prev <= 0.0;
            // A falling gate releases — and so does a gate found *already*
            // closed on the very first sample. A live client's note-on and
            // note-off can land in the same command drain (both applied
            // before the node's first block), so the envelope never sees an
            // edge; without this it would play its segments and sustain
            // forever on a closed gate — a stuck, audible node. Born
            // released, it plays the release segment from the initial level
            // and finishes, so the done action still frees the node.
            let born = !self.primed;
            self.primed = true;
            let release_edge = gate <= 0.0 && (self.gate_prev > 0.0 || born);
            self.gate_prev = gate;

            if trig {
                self.current_segment = 0;
                self.segment_phase = 0;
                self.released = false;
                self.finished = false;
                self.start_level = at(inputs[5], i);
                self.last_val = self.start_level;
            } else if release_edge && !self.finished {
                self.released = true;
                if born {
                    // Never triggered: the level "reached so far" is the
                    // envelope's initial level.
                    self.last_val = at(inputs[5], i);
                }
                if has_release {
                    // Resume from the release segment, starting at the level
                    // reached so far (which may be mid-sustain-decay).
                    self.start_level = self.last_val;
                    self.current_segment = release_node;
                    self.segment_phase = 0;
                }
            }

            if self.finished {
                *out = self.last_val * level_scale + level_bias;
                continue;
            }

            // Advance past every segment whose duration has elapsed.
            loop {
                if self.current_segment >= num_segments {
                    self.finished = true;
                    break;
                }
                // Reached the release node with the gate still open: either
                // loop back (carrying the current level as the loop's start) or
                // sustain by holding here until the gate falls.
                if has_release && self.current_segment == release_node && !self.released {
                    if has_loop {
                        self.current_segment = loop_node;
                        continue;
                    }
                    break;
                }
                let base = HEADER_INPUTS + self.current_segment * SEGMENT_INPUTS;
                if base + SEGMENT_INPUTS > inputs.len() {
                    self.finished = true;
                    break;
                }
                let dur = at(inputs[base + 1], i) * time_scale;
                let dur_samples = (dur * ctx.sample_rate).max(1.0) as usize;
                if self.segment_phase < dur_samples {
                    break;
                }
                // Segment complete: land exactly on its target and step on.
                self.start_level = at(inputs[base], i);
                self.last_val = self.start_level;
                self.current_segment += 1;
                self.segment_phase -= dur_samples;
            }

            if self.finished {
                *out = self.last_val * level_scale + level_bias;
                continue;
            }
            if has_release && self.current_segment == release_node && !self.released {
                // Sustaining at the release level.
                *out = self.last_val * level_scale + level_bias;
                continue;
            }

            // Interpolate within the current segment.
            let base = HEADER_INPUTS + self.current_segment * SEGMENT_INPUTS;
            let target = at(inputs[base], i);
            let dur = at(inputs[base + 1], i) * time_scale;
            let dur_samples = (dur * ctx.sample_rate).max(1.0) as usize;
            let shape = at(inputs[base + 2], i) as i32;
            let curve = at(inputs[base + 3], i);

            let frac = self.segment_phase as f32 / dur_samples as f32;
            let val = shape_value(shape, curve, self.start_level, target, frac);

            self.last_val = val;
            *out = val * level_scale + level_bias;
            self.segment_phase += 1;
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
