//! `LocalIn`/`LocalOut`: synth-private feedback buses with one control block
//! (64 samples) of delay — the UGen-graph counterpart of SuperCollider's
//! `LocalIn`/`LocalOut`.
//!
//! The graph is a DAG, so a feedback loop cannot be wired directly. `LocalIn`
//! is a source (no wire input, ordered first) and `LocalOut` a sink: they do
//! not connect by a wire but through a **per-synth buffer that persists across
//! blocks**, living in [`crate::synthdef::instance::UGenSynth`]. Within a
//! block, `LocalIn` reads the buffer (still holding what `LocalOut` wrote the
//! previous block) before `LocalOut` overwrites it — the one-block delay falls
//! out of the read-before-write order, with no double buffering.
//!
//! Because that buffer is synth-private state the [`UGen`] trait and
//! [`ProcessCtx`] do not expose, the copies are done in `UGenSynth::process`,
//! which intercepts these two kinds. These structs only exist so the registry
//! can build them; their `process` is never called.
//!
//! This is **block-rate** feedback (good for feedback delays/reverbs, block
//! feedback-FM, networked loops). Sample-accurate (sub-block) feedback needs
//! the whole loop fused into one node: a recursive single-UGen filter or a
//! Faust def.

use crate::dsp::{ProcessCtx, UGen};

/// Reads synth-local feedback channel (input 0: the channel index, constant).
pub struct LocalIn;

/// Writes its signal (input 1) to synth-local feedback channel (input 0).
pub struct LocalOut;

impl UGen for LocalIn {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        debug_assert!(false, "LocalIn must be intercepted by UGenSynth::process");
        output.fill(0.0); // safe fallback should this ever run in release
    }
}

impl UGen for LocalOut {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        debug_assert!(false, "LocalOut must be intercepted by UGenSynth::process");
        output.fill(0.0);
    }
}
