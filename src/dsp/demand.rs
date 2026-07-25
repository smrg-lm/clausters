//! Demand-rate (`dr`) infrastructure (S1): the pull protocol that the whole
//! demand family (`Dseries`/`Dwhite`/`Duty`/`TDuty`, later) will build on.
//!
//! A demand UGen is **not** in the normal block-execution order. It is a
//! *sub-list* its driver owns: the driver ([`Demand`]) pulls one value at a
//! time via [`UGen::demand`], the source ([`Dseq`]) yields the next value of
//! its stream on each pull. The synth wires the two together in
//! [`crate::synthdef::instance`] — the source is skipped in block order and
//! reached only through the driver's `step` callback, so there is exactly one
//! mutable path to it (no aliasing) and no allocation on the audio thread.
//!
//! This is a deliberately **minimal** driver — enough to prove the contract:
//! end-of-stream is signalled by a `NaN` pull and the driver holds its last
//! value. The `dr` families that land later reuse these same two trait hooks.

use crate::dsp::trig::Edge;
use crate::dsp::{ProcessCtx, UGen, at};

/// `Dseq(repeats, v0, v1, …)`: a demand *source* that yields its value list in
/// order, `repeats` times (`repeats <= 0` loops forever), then yields `NaN`
/// (stream exhausted). Input 0 is `repeats`; inputs `1..` are the values.
/// Produces nothing in block order — its driver pulls it.
pub struct Dseq {
    /// Index of the next value to yield within the list.
    pos: usize,
    /// Completed passes over the list.
    passes: u32,
}

impl Dseq {
    pub fn new() -> Self {
        Self { pos: 0, passes: 0 }
    }
}

impl Default for Dseq {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Dseq {
    // Demand sources are skipped in block order; this is never called.
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &[&[f32]]) -> f32 {
        let repeats = inputs[0][0];
        let values = &inputs[1..];
        if values.is_empty() {
            return f32::NAN;
        }
        // Finite repeat count exhausted?
        if repeats > 0.0 && self.passes >= repeats as u32 {
            return f32::NAN;
        }
        let v = at(values[self.pos], 0);
        self.pos += 1;
        if self.pos >= values.len() {
            self.pos = 0;
            self.passes = self.passes.saturating_add(1);
        }
        v
    }

    fn reset_demand(&mut self) {
        self.pos = 0;
        self.passes = 0;
    }
}

/// `Demand(trig, reset, source)`: the demand *driver*. On each rising edge of
/// `trig` it pulls the next value from `source` and holds it until the next
/// trigger; a rising edge of `reset` resets the source's stream. The output is
/// `0` before the first trigger, and holds the last value when the source is
/// exhausted (a `NaN` pull). Audio or control rate.
pub struct Demand {
    held: f32,
    prev_trig: Edge,
    prev_reset: Edge,
}

impl Demand {
    pub fn new() -> Self {
        Self {
            held: 0.0,
            prev_trig: Edge::default(),
            prev_reset: Edge::default(),
        }
    }
}

impl Default for Demand {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Demand {
    // The synth drives this via `drive`; `process` is never called.
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn drive(
        &mut self,
        trig: &[f32],
        reset: &[f32],
        output: &mut [f32],
        step: &mut dyn FnMut(bool) -> f32,
    ) {
        for (i, out) in output.iter_mut().enumerate() {
            if self.prev_reset.rose(at(reset, i)) {
                step(true); // reset the source's stream
            }
            if self.prev_trig.rose(at(trig, i)) {
                let v = step(false); // pull the next value
                if v.is_finite() {
                    self.held = v; // NaN = exhausted: hold the last value
                }
            }
            *out = self.held;
        }
    }
}
