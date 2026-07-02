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

use crate::dsp::registry::{
    DEMAND_SOURCE_SLOT, UGenConfig, UGenKind, arity, default_rate, parse_kind, rate_allowed,
};
use crate::dsp::{MAX_UGEN_INPUTS, Rate};

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

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct UGenSpec {
    pub kind: String,
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
    /// Output calculation rate (`ir`/`kr`/`ar`/`dr`). Omitted means the kind's
    /// default (see [`crate::dsp::registry::default_rate`]); the compiler
    /// validates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<String>,
    /// `DiskIn`/`DiskOut`: file path. Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `DiskIn`: restart from the top of the file at end of stream.
    #[serde(default, rename = "loop", skip_serializing_if = "std::ops::Not::not")]
    pub looping: bool,
    /// `DiskOut`: WAV sample format (`int16` | `int24` | `float`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
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
    /// Output calculation rate, inferred and validated at compile time (S1).
    pub rate: Rate,
    /// Static per-UGen parameters (e.g. `DiskIn`/`DiskOut` paths). Default for
    /// every other kind.
    pub config: UGenConfig,
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
    let mut ugens: Vec<UGenDef> = Vec::with_capacity(spec.ugens.len());
    // Synth-private feedback channels: size the buffer and require each
    // channel's LocalIn to precede its LocalOut (the one-block-delay contract).
    let mut num_locals = 0usize;
    let mut localin_channels = std::collections::HashSet::new();

    for (i, u) in spec.ugens.iter().enumerate() {
        let kind =
            parse_kind(&u.kind).ok_or_else(|| format!("ugens[{i}]: unknown kind '{}'", u.kind))?;
        let want = arity(kind);
        if want != usize::MAX && u.inputs.len() != want {
            return Err(format!(
                "ugens[{i}] ({}): expected {want} inputs, got {}",
                u.kind,
                u.inputs.len()
            ));
        }
        if u.inputs.len() > MAX_UGEN_INPUTS {
            return Err(format!(
                "ugens[{i}] ({}): inputs ({}) exceed MAX_UGEN_INPUTS ({MAX_UGEN_INPUTS})",
                u.kind,
                u.inputs.len()
            ));
        }

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

        // DiskIn/DiskOut carry a file path as a static parameter; require it
        // at compile time so a bad def fails fast with `/fail`.
        if matches!(kind, UGenKind::DiskIn | UGenKind::DiskOut)
            && u.path.as_deref().is_none_or(str::is_empty)
        {
            return Err(format!(
                "ugens[{i}] ({}): requires a non-empty path",
                u.kind
            ));
        }
        let config = UGenConfig {
            path: u.path.clone(),
            looping: u.looping,
            format: u.format.clone(),
        };

        let mut inputs = Vec::with_capacity(u.inputs.len());
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

        // Output rate (S1): the explicit `rate` field validated against the
        // kind, or the kind's default. `ugens` already holds every earlier
        // UGenDef, so a wire's producer rate is known here.
        let rate = match &u.rate {
            Some(name) => {
                let r = Rate::parse(name)
                    .ok_or_else(|| format!("ugens[{i}] ({}): unknown rate '{name}'", u.kind))?;
                if !rate_allowed(kind, r) {
                    return Err(format!(
                        "ugens[{i}] ({}): rate '{}' is not allowed for this kind",
                        u.kind,
                        r.as_str()
                    ));
                }
                r
            }
            None => default_rate(kind),
        };

        // Rate coercion (S1). Lower rates widen into higher-rate inputs for
        // free, so the only illegal narrowings are:
        //  - an `ir` UGen with a non-`ir` input (it is computed once at init —
        //    a varying source cannot be frozen);
        //  - demand rate crossing the block boundary: a `dr` wire may only
        //    feed a demand driver's source slot, and that slot must be `dr`.
        for (k, r) in inputs.iter().enumerate() {
            let in_rate = match r {
                InputRef::Const(_) => Rate::Ir,
                InputRef::Control(_) => Rate::Kr,
                InputRef::Wire(w) => ugens[*w].rate,
            };
            let is_demand_slot = kind == UGenKind::Demand && k == DEMAND_SOURCE_SLOT;
            if in_rate == Rate::Dr {
                if !is_demand_slot {
                    return Err(format!(
                        "ugens[{i}] ({}).inputs[{k}]: a demand-rate (dr) signal can only feed a \
                         demand driver's source",
                        u.kind
                    ));
                }
            } else if is_demand_slot {
                return Err(format!(
                    "ugens[{i}] (Demand).inputs[{k}]: the demand source must be a demand-rate \
                     (dr) UGen"
                ));
            } else if rate == Rate::Ir && in_rate != Rate::Ir {
                return Err(format!(
                    "ugens[{i}] ({}).inputs[{k}]: an ir-rate UGen requires ir inputs, got {}",
                    u.kind,
                    in_rate.as_str()
                ));
            }
        }

        ugens.push(UGenDef {
            kind,
            inputs,
            rate,
            config,
        });
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
                ..Default::default()
            },
            UGenSpec {
                kind: "Mul".into(),
                inputs: vec![InputSpec::Ugen(0), InputSpec::Control(1)],
                ..Default::default()
            },
            UGenSpec {
                kind: "Out".into(),
                inputs: vec![InputSpec::Const(0.0), InputSpec::Ugen(1)],
                ..Default::default()
            },
            UGenSpec {
                kind: "Out".into(),
                inputs: vec![InputSpec::Const(1.0), InputSpec::Ugen(1)],
                ..Default::default()
            },
        ],
    }
}
