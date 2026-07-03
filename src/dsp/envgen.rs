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

use crate::dsp::{DoneAction, ProcessCtx, UGen, at};

/// SuperCollider envelope shape number, `t` in `[0, 1)` the position within the
/// segment, `a` the start level, `b` the target, `c` the curve value (only used
/// by the custom-curvature shape). Returns the interpolated level.
///
/// The endpoints hold: every shape yields `a` at `t == 0` and tends to `b` as
/// `t -> 1`; the exact target is committed by the caller when the segment
/// completes, so `t` never actually reaches 1.
#[inline]
fn shape_value(shape: i32, c: f32, a: f32, b: f32, t: f32) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI};
    match shape {
        // Step: jump to the target immediately and hold it for the duration.
        0 => b,
        // Hold: stay at the start level; the jump to the target happens when
        // the segment completes.
        8 => a,
        // Exponential: needs same-sign, non-zero levels; a crossing through or
        // to zero is undefined, so nudge zeros to a tiny same-signed value and
        // fall back to linear across a sign change.
        2 => {
            let a = if a.abs() < 1e-5 {
                1e-5_f32.copysign(a)
            } else {
                a
            };
            let b = if b.abs() < 1e-5 {
                1e-5_f32.copysign(b)
            } else {
                b
            };
            if a.signum() == b.signum() {
                a * (b / a).powf(t)
            } else {
                a + t * (b - a)
            }
        }
        // Sine: equal-power ease in/out (half a cosine).
        3 => a + (b - a) * (1.0 - (PI * t).cos()) * 0.5,
        // Welch: a quarter sine, concave for a rise and convex for a fall.
        4 => {
            if b >= a {
                a + (b - a) * (FRAC_PI_2 * t).sin()
            } else {
                b + (a - b) * (FRAC_PI_2 * (1.0 - t)).sin()
            }
        }
        // Custom curvature: `c` bends the segment (0 == linear, positive builds
        // slowly then fast, negative the reverse).
        5 => {
            if c.abs() < 0.001 {
                a + t * (b - a)
            } else {
                a + (b - a) * (1.0 - (t * c).exp()) / (1.0 - c.exp())
            }
        }
        // Squared / cubed: interpolate the square/cube root linearly, then raise
        // back. Squared clamps to non-negative levels (its root is real only
        // there); cubed uses the sign-preserving real cube root.
        6 => {
            let ra = a.max(0.0).sqrt();
            let rb = b.max(0.0).sqrt();
            let s = ra + t * (rb - ra);
            s * s
        }
        7 => {
            let ra = a.cbrt();
            let rb = b.cbrt();
            let s = ra + t * (rb - ra);
            s * s * s
        }
        // Linear (1) and any unknown shape.
        _ => a + t * (b - a),
    }
}

/// Number of leading, non-segment inputs (see the layout comment in `process`).
const HEADER_INPUTS: usize = 9;
/// Inputs per segment: target, duration, shape, curve.
const SEGMENT_INPUTS: usize = 4;

pub struct EnvGen {
    current_segment: usize,
    segment_phase: usize,
    gate_prev: f32,
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
            let release_edge = gate <= 0.0 && self.gate_prev > 0.0;
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
}
