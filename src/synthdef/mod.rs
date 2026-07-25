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
//!   "name": "sine",
//!   "controls": [
//!     {"name": "freq", "default": 440.0},
//!     {"name": "amp",  "default": 0.2}
//!   ],
//!   "ugens": [
//!     {"kind": "Sine", "inputs": [{"control": 0}]},
//!     {"kind": "Mul",    "inputs": [{"ugen": 0}, {"control": 1}]},
//!     {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 1}]}
//!   ]
//! }
//! ```
//!
//! The built-in `"default"` def ([`default_spec`]) is this same shape plus a
//! gated envelope (see that function).

pub mod instance;

use serde::{Deserialize, Serialize};

use clausters_core::{builtins, pvprog};

use crate::dsp::registry::{
    Arity, DEMAND_SOURCE_SLOT, ExecMode, OpFamily, SpectralRole, UGenConfig, UGenDescriptor, lookup,
};
use crate::dsp::spectral::resolve_fft_size;
use crate::dsp::{MAX_UGEN_INPUTS, Rate};
use clausters_core::fft;

// ---- wire format (serde) ----

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SynthDefSpec {
    pub name: String,
    #[serde(default)]
    pub controls: Vec<ControlSpec>,
    pub ugens: Vec<UGenSpec>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ControlSpec {
    pub name: String,
    pub default: f32,
    /// Control type (S2): `"kr"` (default, a plain control), `"tr"` (a
    /// one-block trigger the engine resets to 0), or `"ir"` (scalar, read once
    /// at init and frozen). Omitted means `"kr"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<String>,
    /// Lag time in seconds (S2): a `kr` control whose changes are smoothed by
    /// an implicit one-pole `Lag` (or `VarLag` with `lag_down`) inserted at
    /// compile time. `None` (or `0`) means no smoothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag: Option<f32>,
    /// Separate downward lag time (S2): when set alongside `lag`, the control
    /// smooths with `VarLag` (`lag` up, `lag_down` down).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag_down: Option<f32>,
}

/// A control's type (S2): scsynth's control rates for SynthDef controls. The
/// lag time is separate (it compiles to an inserted `Lag`, not a type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ControlType {
    /// A plain control, read once per block, settable any time (`kr`).
    #[default]
    Control,
    /// A one-block trigger: a `/n_set` holds for one block, then the engine
    /// resets it to 0 (`tr`).
    Trigger,
    /// A scalar read once at init and frozen; a later `/n_set` is ignored
    /// (`ir`, pairing with S1's `ir` rate).
    Scalar,
}

impl ControlType {
    fn parse(name: &str) -> Option<ControlType> {
        match name {
            "kr" | "control" => Some(ControlType::Control),
            "tr" | "trigger" => Some(ControlType::Trigger),
            "ir" | "scalar" => Some(ControlType::Scalar),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct UGenSpec {
    pub kind: String,
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
    /// Output calculation rate (`ir`/`kr`/`ar`/`dr`). Omitted means the kind's
    /// default (its [`UGenDescriptor::default_rate`]); the compiler validates
    /// it against the descriptor's allowed rates.
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
    /// `BinaryOpUGen`/`UnaryOpUGen`: the operator, by **name** (`"mul"`,
    /// `"midicps"`, `"clip2"`, …). Ignored by every other kind. The compiler
    /// resolves it against the shared `clausters_core::builtins` operator table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// Side-effect UGens (S9): `SendReply`'s command name (the OSC address it
    /// replies with, default `/reply`) or `Poll`'s label (default `poll`).
    /// Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Spectral chain (S8): `FFT` window size, a supported power of two. Given
    /// only on the `FFT`; the compiler propagates it to the rest of the chain.
    /// Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fft_size: Option<usize>,
    /// Spectral chain (S8): `FFT` hop as a fraction of the window (default
    /// `0.5`). Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop: Option<f32>,
    /// Spectral chain (S8): `FFT`/`IFFT` window type (default `0`, Hann).
    /// Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wintype: Option<i32>,
    /// `Conv` (M28): maximum partition count (FDL capacity — the longest
    /// prepared kernel the instance accepts). Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partitions: Option<usize>,
    /// Delay family (U3): the longest delay the instance accepts, in seconds.
    /// It sizes the pre-allocated line, so it is static config rather than a
    /// signal input. Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay: Option<f32>,
    /// `PV_Kernel`: the magnitude bin-expression as a **postfix token list** —
    /// a number pushes a constant, a word is a per-bin load (`"mag"`,
    /// `"phase"`, `"bin"`, `"nbins"`, `"binfreq"`, `"p0"`…) or an operator wire
    /// name from the shared `clausters_core::builtins` tables (`"mul"`,
    /// `"ge"`, `"tanh"`, …). Omitted means the identity (`["mag"]`). Validated
    /// at compile time (see `clausters_core::pvprog`). Ignored by every other
    /// kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mag_expr: Option<Vec<PvTokenSpec>>,
    /// `PV_Kernel`: the phase bin-expression, same token format as
    /// `mag_expr`. Omitted means the identity (`["phase"]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_expr: Option<Vec<PvTokenSpec>>,
}

/// One wire token of a `PV_Kernel` bin expression: a literal number (pushes a
/// constant) or a word (a load or an operator name — see
/// [`clausters_core::pvprog::parse_word`]).
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum PvTokenSpec {
    Num(f32),
    Word(String),
}

/// Resolves a `PV_Kernel` token list into a validated program. `what` names
/// the field for the error message; `n_params` is the UGen's parameter-input
/// count (inputs past the chain).
fn compile_pv_expr(
    tokens: &[PvTokenSpec],
    n_params: usize,
    what: &str,
) -> Result<pvprog::PvProgram, String> {
    let mut ops = Vec::with_capacity(tokens.len());
    for t in tokens {
        ops.push(match t {
            PvTokenSpec::Num(x) => {
                if !x.is_finite() {
                    return Err(format!("{what}: non-finite constant"));
                }
                pvprog::PvOp::Const(*x)
            }
            PvTokenSpec::Word(w) => {
                pvprog::parse_word(w).ok_or_else(|| format!("{what}: unknown word '{w}'"))?
            }
        });
    }
    pvprog::compile(ops, n_params).map_err(|e| format!("{what}: {e}"))
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

pub struct UGenDef {
    /// The catalog descriptor for this UGen's kind (its name, arity, rates,
    /// bus role, execution mode and constructor) — the compiler and engine
    /// read it instead of matching on a kind enum.
    pub desc: &'static UGenDescriptor,
    pub inputs: Vec<InputRef>,
    /// Output calculation rate, inferred and validated at compile time (S1).
    pub rate: Rate,
    /// Static per-UGen parameters (e.g. `DiskIn`/`DiskOut` paths). Default for
    /// every other kind.
    pub config: UGenConfig,
    /// Spectral-chain slot (S8): which synth-private
    /// [`SpectralChain`](crate::dsp::spectral::SpectralChain) this UGen shares.
    /// Assigned by the compiler — a fresh slot for each `FFT`, inherited by the
    /// `PV_*`/`IFFT` downstream. `None` for every non-spectral UGen.
    pub chain_slot: Option<usize>,
    /// Second chain slot of a two-chain combiner (`SpectralRole::Filter2`,
    /// M27): the chain read as input 1 (chain B). `None` everywhere else.
    pub chain_slot_b: Option<usize>,
}

impl std::fmt::Debug for UGenDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UGenDef")
            .field("kind", &self.desc.name)
            .field("inputs", &self.inputs)
            .field("rate", &self.rate)
            .field("config", &self.config)
            .finish()
    }
}

#[derive(Debug)]
pub struct SynthDef {
    pub name: String,
    pub control_names: Vec<String>,
    pub control_defaults: Vec<f32>,
    /// Control types parallel to `control_names` (S2): trigger controls the
    /// engine resets each block, scalar controls it freezes after init.
    pub control_types: Vec<ControlType>,
    pub constants: Vec<f32>,
    /// Topologically ordered: inputs only reference earlier UGens.
    pub ugens: Vec<UGenDef>,
    /// Number of synth-private feedback channels (`LocalIn`/`LocalOut`); the
    /// instance allocates this many persistent `Block`s. 0 for most defs.
    pub num_locals: usize,
    /// One entry per spectral chain (S8), its FFT window size; the instance
    /// allocates a [`SpectralChain`](crate::dsp::spectral::SpectralChain) of
    /// each size. Empty for defs with no `FFT`.
    pub spectral_sizes: Vec<usize>,
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
    // Spectral chains (S8): each `FFT` opens one; its window size is recorded
    // here and its slot index is `spectral_sizes.len()` at that point.
    let mut spectral_sizes: Vec<usize> = Vec::new();

    // Control types + lag times (S2). `lagged` collects (control index, up
    // time, optional down time) for the compile-time Lag insertion below.
    let mut control_types = Vec::with_capacity(n_controls);
    let mut lagged: Vec<(usize, f32, Option<f32>)> = Vec::new();
    for (ci, c) in spec.controls.iter().enumerate() {
        let ty = match &c.rate {
            Some(name) => ControlType::parse(name).ok_or_else(|| {
                format!("controls[{ci}] ({}): unknown control type '{name}'", c.name)
            })?,
            None => ControlType::Control,
        };
        if c.lag_down.is_some() && c.lag.is_none() {
            return Err(format!(
                "controls[{ci}] ({}): lag_down requires lag (the up time)",
                c.name
            ));
        }
        let up = c.lag.unwrap_or(0.0);
        let down = c.lag_down;
        if (up > 0.0 || down.is_some_and(|d| d > 0.0)) && ty != ControlType::Control {
            return Err(format!(
                "controls[{ci}] ({}): lag is only valid on a kr (plain) control",
                c.name
            ));
        }
        if up > 0.0 || down.is_some_and(|d| d > 0.0) {
            lagged.push((ci, up.max(0.0), down));
        }
        control_types.push(ty);
    }

    for (i, u) in spec.ugens.iter().enumerate() {
        let desc =
            lookup(&u.kind).ok_or_else(|| format!("ugens[{i}]: unknown kind '{}'", u.kind))?;
        if let Arity::Fixed(want) = desc.arity
            && u.inputs.len() != want
        {
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
        if matches!(desc.exec, ExecMode::LocalIn | ExecMode::LocalOut) {
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
            match desc.exec {
                ExecMode::LocalIn => {
                    localin_channels.insert(channel);
                }
                ExecMode::LocalOut if !localin_channels.contains(&channel) => {
                    return Err(format!(
                        "ugens[{i}] (LocalOut): local channel {channel} has no earlier LocalIn; \
                         LocalIn must precede LocalOut (one block of feedback delay)"
                    ));
                }
                _ => {}
            }
        }

        // Some UGens (DiskIn/DiskOut) carry a file path as a static parameter;
        // require it at compile time so a bad def fails fast with `/fail`.
        if desc.needs_path && u.path.as_deref().is_none_or(str::is_empty) {
            return Err(format!(
                "ugens[{i}] ({}): requires a non-empty path",
                u.kind
            ));
        }
        // The generic op UGens carry their operator by name; resolve it against
        // the family's opcode table so a bad def fails fast, and keep the
        // internal numeric index for `build` (the name never reaches the engine).
        let mut op_index = None;
        if let Some(family) = desc.op_family {
            let name = u.op.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| {
                format!("ugens[{i}] ({}): requires an 'op' operator name", u.kind)
            })?;
            let resolved = match family {
                OpFamily::Unary => builtins::UnaryOp::from_name(name).map(|o| o as u32),
                OpFamily::Binary => builtins::BinaryOp::from_name(name).map(|o| o as u32),
            };
            op_index = Some(resolved.ok_or_else(|| {
                format!(
                    "ugens[{i}] ({}): unknown {family:?} operator '{name}'",
                    u.kind
                )
            })?);
        }
        let mut config = UGenConfig {
            path: u.path.clone(),
            looping: u.looping,
            format: u.format.clone(),
            op: op_index,
            label: u.label.clone(),
            fft_size: u.fft_size,
            hop: u.hop,
            wintype: u.wintype,
            partitions: u.partitions,
            mag_prog: None,
            phase_prog: None,
            max_delay: u.max_delay,
        };
        // Any kind that takes an `fft_size` (the spectral chain's FFT, the
        // partitioned convolver) must name a supported transform size.
        if let Some(sz) = u.fft_size
            && !fft::supports(sz)
        {
            return Err(format!(
                "ugens[{i}] ({}): unsupported fft_size {sz}; use one of {:?}",
                u.kind,
                fft::SUPPORTED_SIZES
            ));
        }
        // `PV_Kernel` bin expressions (M29): resolve and validate the postfix
        // token lists now, so the RT thread only ever runs a program that
        // passed the stack/arity checks. The parameters a program may read
        // (`p0`…) are this UGen's inputs past the chain (input 0).
        let n_params = u.inputs.len().saturating_sub(1);
        if let Some(tokens) = &u.mag_expr {
            let what = format!("ugens[{i}] ({}) mag_expr", u.kind);
            config.mag_prog = Some(compile_pv_expr(tokens, n_params, &what)?);
        }
        if let Some(tokens) = &u.phase_expr {
            let what = format!("ugens[{i}] ({}) phase_expr", u.kind);
            config.phase_prog = Some(compile_pv_expr(tokens, n_params, &what)?);
        }

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

        // Spectral chain (S8). A `Source` (`FFT`) opens a new chain: validate
        // its window size and record its slot. A `Filter`/`Sink` (`PV_*`/
        // `IFFT`) must take a spectral wire as input 0 and inherits that chain's
        // slot, window size and (if unset) window type — so the client only
        // specifies the size once, on the `FFT`.
        let mut chain_slot: Option<usize> = None;
        let mut chain_slot_b: Option<usize> = None;
        // Resolves spectral input `k` to the chain slot it carries.
        let chain_of =
            |k: usize, inputs: &[InputRef], ugens: &[UGenDef]| -> Result<usize, String> {
                // Guards the variadic spectral kinds (`PV_Kernel`), whose
                // fixed-arity check above does not run.
                if k >= inputs.len() {
                    return Err(format!(
                        "ugens[{i}] ({}): missing input {k} (the spectral chain)",
                        u.kind
                    ));
                }
                let InputRef::Wire(w) = inputs[k] else {
                    return Err(format!(
                        "ugens[{i}] ({}): input {k} must be the spectral chain from an earlier \
                     FFT/PV_* UGen",
                        u.kind
                    ));
                };
                ugens[w].chain_slot.ok_or_else(|| {
                    format!(
                        "ugens[{i}] ({}): input {k} (ugen {w}, {}) is not a spectral chain",
                        u.kind, ugens[w].desc.name
                    )
                })
            };
        match desc.spectral {
            SpectralRole::None => {}
            SpectralRole::Source => {
                let winsize = resolve_fft_size(u.fft_size);
                config.fft_size = Some(winsize);
                chain_slot = Some(spectral_sizes.len());
                spectral_sizes.push(winsize);
            }
            SpectralRole::Filter | SpectralRole::Sink => {
                let slot = chain_of(0, &inputs, &ugens)?;
                let InputRef::Wire(w) = inputs[0] else {
                    unreachable!("chain_of validated the wire")
                };
                let up = &ugens[w];
                chain_slot = Some(slot);
                config.fft_size = Some(spectral_sizes[slot]);
                if config.wintype.is_none() {
                    config.wintype = up.config.wintype;
                }
                if config.hop.is_none() {
                    config.hop = up.config.hop;
                }
            }
            // A two-chain combiner (M27): inputs 0 and 1 are chains of equal
            // window size and distinct slots; the result lands in chain A, so
            // the combiner inherits A's slot (a downstream filter/sink then
            // reads the combined chain through it).
            SpectralRole::Filter2 => {
                let a = chain_of(0, &inputs, &ugens)?;
                let b = chain_of(1, &inputs, &ugens)?;
                if a == b {
                    return Err(format!(
                        "ugens[{i}] ({}): both inputs read the same spectral chain",
                        u.kind
                    ));
                }
                if spectral_sizes[a] != spectral_sizes[b] {
                    return Err(format!(
                        "ugens[{i}] ({}): chain window sizes differ ({} vs {})",
                        u.kind, spectral_sizes[a], spectral_sizes[b]
                    ));
                }
                chain_slot = Some(a);
                chain_slot_b = Some(b);
                config.fft_size = Some(spectral_sizes[a]);
            }
        }

        // Output rate (S1): the explicit `rate` field validated against the
        // kind, or the kind's default. `ugens` already holds every earlier
        // UGenDef, so a wire's producer rate is known here.
        let rate = match &u.rate {
            Some(name) => {
                let r = Rate::parse(name)
                    .ok_or_else(|| format!("ugens[{i}] ({}): unknown rate '{name}'", u.kind))?;
                if !desc.allows(r) {
                    return Err(format!(
                        "ugens[{i}] ({}): rate '{}' is not allowed for this kind",
                        u.kind,
                        r.as_str()
                    ));
                }
                r
            }
            None => desc.default_rate,
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
                // A scalar (`ir`) control is init-rate; every other control is
                // control-rate (S2 pairs `ir` controls with S1's `ir` rate).
                InputRef::Control(c) if control_types[*c] == ControlType::Scalar => Rate::Ir,
                InputRef::Control(_) => Rate::Kr,
                InputRef::Wire(w) => ugens[*w].rate,
            };
            let is_demand_slot = desc.exec == ExecMode::DemandDriver && k == DEMAND_SOURCE_SLOT;
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

        // `Done`/`FreeSelfWhenDone` watch another UGen's *done flag*, which is
        // not a value on a wire: input 0 must therefore name a UGen, and one
        // that can finish at all. Rejecting it here turns a UGen that would
        // have read zero for the node's whole life into a pointed error.
        if desc.exec == ExecMode::DoneQuery {
            match inputs.first() {
                Some(InputRef::Wire(w)) if ugens[*w].desc.has_done_flag => {}
                Some(InputRef::Wire(w)) => {
                    return Err(format!(
                        "ugens[{i}] ({}).inputs[0]: {} has no done flag — only a UGen that \
                         finishes (an envelope) can be watched",
                        u.kind, ugens[*w].desc.name
                    ));
                }
                _ => {
                    return Err(format!(
                        "ugens[{i}] ({}).inputs[0]: must be another UGen (an envelope), not a \
                         constant or a control",
                        u.kind
                    ));
                }
            }
        }

        ugens.push(UGenDef {
            desc,
            inputs,
            rate,
            config,
            chain_slot,
            chain_slot_b,
        });
    }

    // Compile-time lag insertion (S2): a lagged control compiles to a `Lag`
    // (or `VarLag`) UGen reading the raw control, prepended to the graph; every
    // reference to that control is rewritten to the smoother's output. Reusing
    // the real UGen keeps a single lag implementation shared with the library
    // (no bespoke control path). The smoothers run at audio rate, so a lagged
    // control glides per sample toward its block-constant target.
    if !lagged.is_empty() {
        let n_lag = lagged.len();
        let lag_desc = lookup("Lag").expect("Lag is registered");
        let varlag_desc = lookup("VarLag").expect("VarLag is registered");
        // control index -> its lag UGen's position (0..n_lag), prepended.
        let mut lag_pos: Vec<Option<usize>> = vec![None; n_controls];
        let mut prefix: Vec<UGenDef> = Vec::with_capacity(n_lag);
        for (pos, &(ci, up, down)) in lagged.iter().enumerate() {
            lag_pos[ci] = Some(pos);
            constants.push(up);
            let up_ref = InputRef::Const(constants.len() - 1);
            let (desc, lag_inputs) = match down {
                Some(d) => {
                    constants.push(d);
                    let down_ref = InputRef::Const(constants.len() - 1);
                    (varlag_desc, vec![InputRef::Control(ci), up_ref, down_ref])
                }
                None => (lag_desc, vec![InputRef::Control(ci), up_ref]),
            };
            prefix.push(UGenDef {
                desc,
                inputs: lag_inputs,
                rate: Rate::Ar,
                config: UGenConfig::default(),
                chain_slot: None,
                chain_slot_b: None,
            });
        }
        // Original UGens shift down by `n_lag`; their wire refs shift too, and
        // a reference to a lagged control becomes the smoother's wire.
        let remapped = ugens.into_iter().map(|u| UGenDef {
            desc: u.desc,
            rate: u.rate,
            config: u.config,
            chain_slot: u.chain_slot,
            chain_slot_b: u.chain_slot_b,
            inputs: u
                .inputs
                .into_iter()
                .map(|r| match r {
                    InputRef::Wire(w) => InputRef::Wire(w + n_lag),
                    InputRef::Control(c) => match lag_pos[c] {
                        Some(pos) => InputRef::Wire(pos),
                        None => InputRef::Control(c),
                    },
                    other => other,
                })
                .collect(),
        });
        prefix.extend(remapped);
        ugens = prefix;
    }

    Ok(SynthDef {
        name: spec.name,
        control_names: spec.controls.iter().map(|c| c.name.clone()).collect(),
        control_defaults: spec.controls.iter().map(|c| c.default).collect(),
        control_types,
        constants,
        ugens,
        num_locals,
        spectral_sizes,
    })
}

/// The built-in "default" def, registered at startup:
/// `Sine(freq) * EnvGen(gate) * amp` to buses 0 and 1 (the hardware outputs).
/// Built through the same spec/compile path as client-sent defs.
///
/// The envelope is a gated ASR (equal-power sine ramps: 0.01 s attack, sustain
/// at 1.0 while the gate is held, 0.3 s release) with `doneAction = FREE_SELF`,
/// so the note ramps in and out without a click and frees itself once the
/// release finishes. A rising `gate` (re)triggers; a `gate 0` starts the
/// release. The client releases this instrument via the gate (see the Python
/// player's `play_event`), which is what lets the release ring out.
pub fn default_spec() -> SynthDefSpec {
    // EnvGen input layout (see `dsp::envgen::EnvGen::process`):
    //   gate, levelScale, levelBias, timeScale, doneAction,
    //   initLevel, numSegments, releaseNode, loopNode,
    //   then [target, duration, shape, curve] per segment.
    // Shape 3 is the equal-power sine curve (`envshape::SHAPE_SINE`); curve is
    // unused for that shape. doneAction 2 is FREE_SELF.
    const SINE: f32 = 3.0;
    let env = UGenSpec {
        kind: "EnvGen".into(),
        inputs: vec![
            InputSpec::Control(2),  // gate
            InputSpec::Const(1.0),  // levelScale
            InputSpec::Const(0.0),  // levelBias
            InputSpec::Const(1.0),  // timeScale
            InputSpec::Const(2.0),  // doneAction = FREE_SELF
            InputSpec::Const(0.0),  // initLevel
            InputSpec::Const(2.0),  // numSegments
            InputSpec::Const(1.0),  // releaseNode (sustain at level index 1)
            InputSpec::Const(-1.0), // loopNode (none)
            // attack: 0 -> 1 over 0.01 s
            InputSpec::Const(1.0),
            InputSpec::Const(0.01),
            InputSpec::Const(SINE),
            InputSpec::Const(0.0),
            // release: 1 -> 0 over 0.3 s
            InputSpec::Const(0.0),
            InputSpec::Const(0.3),
            InputSpec::Const(SINE),
            InputSpec::Const(0.0),
        ],
        ..Default::default()
    };
    SynthDefSpec {
        name: "default".into(),
        controls: vec![
            ControlSpec {
                name: "freq".into(),
                default: 440.0,
                ..Default::default()
            },
            ControlSpec {
                name: "amp".into(),
                default: 0.2,
                ..Default::default()
            },
            ControlSpec {
                name: "gate".into(),
                default: 1.0,
                ..Default::default()
            },
        ],
        ugens: vec![
            UGenSpec {
                kind: "Sine".into(),
                inputs: vec![InputSpec::Control(0)],
                ..Default::default()
            },
            env,
            UGenSpec {
                kind: "Mul".into(),
                inputs: vec![InputSpec::Ugen(0), InputSpec::Ugen(1)],
                ..Default::default()
            },
            UGenSpec {
                kind: "Mul".into(),
                inputs: vec![InputSpec::Ugen(2), InputSpec::Control(1)],
                ..Default::default()
            },
            UGenSpec {
                kind: "Out".into(),
                inputs: vec![InputSpec::Const(0.0), InputSpec::Ugen(3)],
                ..Default::default()
            },
            UGenSpec {
                kind: "Out".into(),
                inputs: vec![InputSpec::Const(1.0), InputSpec::Ugen(3)],
                ..Default::default()
            },
        ],
    }
}
