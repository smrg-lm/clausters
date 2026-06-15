//! SynthDefs: our own definition format (JSON via serde, not SC's binary
//! `.scsyndef`), the compiler that validates it, and the runtime instance.
//!
//! A [`SynthDefSpec`] arrives in `/d_recv` as a JSON blob, is compiled into a
//! [`SynthDef`] (resolved input references, gathered constants) and stored on
//! the network thread. `/s_new` builds a [`instance::UGenSynth`] from it —
//! fully allocated on the network thread — and ships it to the audio thread.
//!
//! Output happens exclusively through `Out`/`ReplaceOut` UGens writing to
//! buses; a def without them is silent. Example of the wire format:
//!
//! ```json
//! {
//!   "name": "default",
//!   "controls": [
//!     {"name": "freq", "default": 440.0},
//!     {"name": "amp",  "default": 0.2}
//!   ],
//!   "ugens": [
//!     {"kind": "SinOsc", "inputs": [{"control": 0}]},
//!     {"kind": "Mul",    "inputs": [{"ugen": 0}, {"control": 1}]},
//!     {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 1}]}
//!   ]
//! }
//! ```

pub mod instance;

use serde::{Deserialize, Serialize};

use crate::dsp::MAX_UGEN_INPUTS;
use crate::dsp::registry::{UGenKind, arity, parse_kind};

// ---- wire format (serde) ----

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SynthDefSpec {
    pub name: String,
    #[serde(default)]
    pub controls: Vec<ControlSpec>,
    pub ugens: Vec<UGenSpec>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ControlSpec {
    pub name: String,
    pub default: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UGenSpec {
    pub kind: String,
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
}

/// An input is a constant, a named control, or the output of an earlier UGen.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum InputSpec {
    Const(f32),
    Control(u32),
    Ugen(u32),
}

// ---- compiled form ----

#[derive(Clone, Copy, Debug)]
pub enum InputRef {
    Const(usize),
    Control(usize),
    Wire(usize),
}

#[derive(Debug)]
pub struct UGenDef {
    pub kind: UGenKind,
    pub inputs: Vec<InputRef>,
}

#[derive(Debug)]
pub struct SynthDef {
    pub name: String,
    pub control_names: Vec<String>,
    pub control_defaults: Vec<f32>,
    pub constants: Vec<f32>,
    /// Topologically ordered: inputs only reference earlier UGens.
    pub ugens: Vec<UGenDef>,
    /// Number of synth-private feedback channels (`LocalIn`/`LocalOut`); the
    /// instance allocates this many persistent `Block`s. 0 for most defs.
    pub num_locals: usize,
}

impl SynthDef {
    pub fn control_index(&self, name: &str) -> Option<u32> {
        self.control_names
            .iter()
            .position(|n| n == name)
            .map(|i| i as u32)
    }
}

/// Validates a spec and resolves it into its compiled form. Errors are meant
/// to travel back to the client in `/fail`, so they name the offending node.
pub fn compile(spec: SynthDefSpec) -> Result<SynthDef, String> {
    if spec.name.is_empty() {
        return Err("empty synthdef name".into());
    }
    if spec.ugens.is_empty() {
        return Err("synthdef has no ugens".into());
    }
    let n_controls = spec.controls.len();
    let mut constants = Vec::new();
    let mut ugens = Vec::with_capacity(spec.ugens.len());
    // Synth-private feedback channels: size the buffer and require each
    // channel's LocalIn to precede its LocalOut (the one-block-delay contract).
    let mut num_locals = 0usize;
    let mut localin_channels = std::collections::HashSet::new();

    for (i, u) in spec.ugens.iter().enumerate() {
        let kind = parse_kind(&u.kind)
            .ok_or_else(|| format!("ugens[{i}]: unknown kind '{}'", u.kind))?;
        let want = arity(kind);
        if u.inputs.len() != want {
            return Err(format!(
                "ugens[{i}] ({}): expected {want} inputs, got {}",
                u.kind,
                u.inputs.len()
            ));
        }
        debug_assert!(want <= MAX_UGEN_INPUTS);

        // LocalIn/LocalOut: channel index (input 0) must be a constant so the
        // buffer can be sized and routed at compile time.
        if matches!(kind, UGenKind::LocalIn | UGenKind::LocalOut) {
            let channel = match u.inputs[0] {
                InputSpec::Const(x) if x.is_finite() && x >= 0.0 => x as usize,
                _ => {
                    return Err(format!(
                        "ugens[{i}] ({}): channel index (input 0) must be a non-negative constant",
                        u.kind
                    ));
                }
            };
            num_locals = num_locals.max(channel + 1);
            match kind {
                UGenKind::LocalIn => {
                    localin_channels.insert(channel);
                }
                UGenKind::LocalOut if !localin_channels.contains(&channel) => {
                    return Err(format!(
                        "ugens[{i}] (LocalOut): local channel {channel} has no earlier LocalIn; \
                         LocalIn must precede LocalOut (one block of feedback delay)"
                    ));
                }
                _ => {}
            }
        }

        let mut inputs = Vec::with_capacity(want);
        for (k, inp) in u.inputs.iter().enumerate() {
            inputs.push(match *inp {
                InputSpec::Const(x) => {
                    if !x.is_finite() {
                        return Err(format!("ugens[{i}].inputs[{k}]: non-finite constant"));
                    }
                    constants.push(x);
                    InputRef::Const(constants.len() - 1)
                }
                InputSpec::Control(c) => {
                    if c as usize >= n_controls {
                        return Err(format!(
                            "ugens[{i}].inputs[{k}]: control {c} out of range (have {n_controls})"
                        ));
                    }
                    InputRef::Control(c as usize)
                }
                InputSpec::Ugen(w) => {
                    if w as usize >= i {
                        return Err(format!(
                            "ugens[{i}].inputs[{k}]: references ugen {w}; only earlier ugens are allowed"
                        ));
                    }
                    InputRef::Wire(w as usize)
                }
            });
        }
        ugens.push(UGenDef { kind, inputs });
    }

    Ok(SynthDef {
        name: spec.name,
        control_names: spec.controls.iter().map(|c| c.name.clone()).collect(),
        control_defaults: spec.controls.iter().map(|c| c.default).collect(),
        constants,
        ugens,
        num_locals,
    })
}

/// The built-in "default" def, registered at startup: SinOsc(freq) * amp to
/// buses 0 and 1 (the hardware outputs). Built through the same spec/compile
/// path as client-sent defs.
pub fn default_spec() -> SynthDefSpec {
    SynthDefSpec {
        name: "default".into(),
        controls: vec![
            ControlSpec {
                name: "freq".into(),
                default: 440.0,
            },
            ControlSpec {
                name: "amp".into(),
                default: 0.2,
            },
        ],
        ugens: vec![
            UGenSpec {
                kind: "SinOsc".into(),
                inputs: vec![InputSpec::Control(0)],
            },
            UGenSpec {
                kind: "Mul".into(),
                inputs: vec![InputSpec::Ugen(0), InputSpec::Control(1)],
            },
            UGenSpec {
                kind: "Out".into(),
                inputs: vec![InputSpec::Const(0.0), InputSpec::Ugen(1)],
            },
            UGenSpec {
                kind: "Out".into(),
                inputs: vec![InputSpec::Const(1.0), InputSpec::Ugen(1)],
            },
        ],
    }
}
