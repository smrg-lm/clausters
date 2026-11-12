//! A gated sine def for tests that listen for one: `Sine(freq) * EnvGen(gate)
//! * amp` on buses 0 and 1, the envelope a gated ASR on equal-power sine ramps
//! (0.01 s attack, 0.3 s release, `doneAction = FREE_SELF`). Controls `freq`,
//! `amp`, `gate`, in that order.
//!
//! The built-in `default` was exactly this until it became a subtractive
//! voice; a test whose subject is the engine -- the node tree, the scheduler,
//! a bus -- measures this one instead, so a change of the default's tone is
//! never a change of what the engine is checked against.

#![allow(dead_code)]

use std::sync::Arc;

use clausters::synthdef::{SynthDef, SynthDefSpec, compile};

/// The def's name, when it is sent over OSC.
pub const SINE_NAME: &str = "sine";

/// The wire format of the def, as `/def_send synth` carries it (`sine.json`,
/// which the golden scenes read too).
pub const SINE_JSON: &str = include_str!("sine.json");

/// The def's spec.
pub fn sine_spec() -> SynthDefSpec {
    serde_json::from_str(SINE_JSON).expect("the sine def is valid JSON")
}

/// The def, compiled.
pub fn sine_def() -> Arc<SynthDef> {
    Arc::new(compile(sine_spec()).expect("the sine def compiles"))
}
