//! SynthDefs: our own definition format (JSON via serde, not SC's binary
//! `.scsyndef`), the compiler that validates it, and the runtime instance.
//!
//! A [`SynthDefSpec`] arrives in `/def_send synth` as a JSON blob, is compiled into a
//! [`SynthDef`] (resolved input references, gathered constants) and stored on
//! the network thread. `/synth_new` builds a [`instance::UGenSynth`] from it --
//! fully allocated on the network thread -- and ships it to the audio thread.
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
//! The built-in `"default"` def ([`default_spec`]) is written in this same
//! format, in `default.json` beside this module.

pub mod instance;

use serde::{Deserialize, Serialize};

use clausters_core::{builtins, pvprog};

use crate::dsp::registry::{
    Arity, ExecMode, OpFamily, SpectralRole, UGenConfig, UGenDescriptor, lookup,
};
use crate::dsp::spectral::{MIN_HOP, resolve_fft_size, resolve_frame, resolve_hop};
use crate::dsp::{MAX_UGEN_INPUTS, Rate};
use clausters_core::fft;
use clausters_core::window::Window;

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
    /// Control type: `"kr"` (default, a plain control), `"tr"` (a
    /// one-block trigger the engine resets to 0), or `"ir"` (scalar, read once
    /// at init and frozen). Omitted means `"kr"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<String>,
    /// Lag time in seconds: a `kr` control whose changes are smoothed by
    /// an implicit one-pole `Lag` (or `LagUD` with `lag_down`) inserted at
    /// compile time. `None` (or `0`) means no smoothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag: Option<f32>,
    /// Separate downward lag time: when set alongside `lag`, the control
    /// smooths with `LagUD` (`lag` up, `lag_down` down).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag_down: Option<f32>,
    /// Optional generic editor metadata, independent of DSP limits.
    #[serde(flatten)]
    pub display: clausters_core::node_editor::ControlDisplay,
}

/// A control's type: scsynth's control rates for SynthDef controls. The
/// lag time is separate (it compiles to an inserted `Lag`, not a type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ControlType {
    /// A plain control, read once per block, settable any time (`kr`).
    #[default]
    Control,
    /// A one-block trigger: a `/node_set` holds for one block, then the engine
    /// resets it to 0 (`tr`).
    Trigger,
    /// A scalar read once at init and frozen; a later `/node_set` is ignored
    /// (`ir`, pairing with the `ir` rate).
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

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
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
    /// `DiskIn`: the frame of the file reading starts at, `0` for its top.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub start: u64,
    /// `DiskOut`: WAV sample format (`int16` | `int24` | `float`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// `BinaryOpUGen`/`UnaryOpUGen`: the operator, by **name** (`"mul"`,
    /// `"midicps"`, `"clip2"`, ...). Ignored by every other kind. The compiler
    /// resolves it against the shared `clausters_core::builtins` operator table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// `RangeMapUGen`: what an out-of-range input is trimmed to before it is
    /// mapped -- `"minmax"` (the default), `"min"`, `"max"` or `"none"`.
    /// Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    /// Side-effect UGens: `SendReply`'s command name (the OSC address it
    /// replies with, default `/reply`) or `Poll`'s label (default `poll`).
    /// Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Spectral chain: `FFT` window size, a supported power of two. Given
    /// only on the `FFT`; the compiler propagates it to the rest of the chain.
    /// Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fft_size: Option<usize>,
    /// Spectral chain: `FFT` hop as a fraction of the window (default
    /// `0.5`). Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop: Option<f32>,
    /// Spectral chain: `FFT`/`IFFT` window type (default `0`, Hann).
    /// Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wintype: Option<i32>,
    /// Spectral chain: the frame an `FFT` or `FFTTrigger` windows, in samples
    /// (scsynth's `winsize`), at least one block and at most `fft_size`, which
    /// it is zero-padded to; the default is `fft_size`. Given only on the
    /// chain's first UGen, like `hop`. Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winsize: Option<usize>,
    /// `Conv`: maximum partition count (FDL capacity -- the longest
    /// prepared kernel the instance accepts). Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partitions: Option<usize>,
    /// Delay family: the longest delay the instance accepts, in seconds.
    /// It sizes the pre-allocated line, so it is static config rather than a
    /// signal input. Ignored by every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay: Option<f32>,
    /// `PV_Kernel`: the magnitude bin-expression as a **postfix token list** --
    /// a number pushes a constant, a word is a per-bin load (`"mag"`,
    /// `"phase"`, `"bin"`, `"nbins"`, `"binfreq"`, `"p0"`...) or an operator wire
    /// name from the shared `clausters_core::builtins` tables (`"mul"`,
    /// `"ge"`, `"tanh"`, ...). Omitted means the identity (`["mag"]`). Validated
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
/// constant) or a word (a load or an operator name -- see
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
    /// bus role, execution mode and constructor) -- the compiler and engine
    /// read it instead of matching on a kind enum.
    pub desc: &'static UGenDescriptor,
    pub inputs: Vec<InputRef>,
    /// Output calculation rate, inferred and validated at compile time.
    pub rate: Rate,
    /// Static per-UGen parameters (e.g. `DiskIn`/`DiskOut` paths). Default for
    /// every other kind.
    pub config: UGenConfig,
    /// Spectral-chain slot: which synth-private
    /// [`SpectralChain`](crate::dsp::spectral::SpectralChain) this UGen shares.
    /// Assigned by the compiler -- a fresh slot for each `FFT`, inherited by the
    /// `PV_*`/`IFFT` downstream. `None` for every non-spectral UGen.
    pub chain_slot: Option<usize>,
    /// Second chain slot of a two-chain combiner (`SpectralRole::Filter2`,
    /// the chain read as input 1 (chain B). `None` everywhere else.
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
    /// Display descriptions retained from the source definition.
    pub control_display: Vec<clausters_core::node_editor::ControlDisplay>,
    /// Control types parallel to `control_names`: trigger controls the
    /// engine resets each block, scalar controls it freezes after init.
    pub control_types: Vec<ControlType>,
    pub constants: Vec<f32>,
    /// Topologically ordered: inputs only reference earlier UGens.
    pub ugens: Vec<UGenDef>,
    /// Number of synth-private feedback channels (`LocalIn`/`LocalOut`); the
    /// instance allocates this many persistent `Block`s. 0 for most defs.
    pub num_locals: usize,
    /// One entry per spectral chain, its FFT window size; the instance
    /// allocates a [`SpectralChain`](crate::dsp::spectral::SpectralChain) of
    /// each size. Empty for defs with no `FFT`.
    pub spectral_sizes: Vec<usize>,
    /// One entry per `LocalBuf`, its channels and frames; the instance
    /// allocates a buffer of each shape, numbered after the pool in this
    /// order. Empty for defs with none.
    pub local_buffers: Vec<(usize, usize)>,
}

impl SynthDef {
    pub fn control_index(&self, name: &str) -> Option<u32> {
        self.control_names
            .iter()
            .position(|n| n == name)
            .map(|i| i as u32)
    }
}

/// The most samples one `LocalBuf` may hold, all channels: 2^27, half a
/// gigabyte, which a def's typo must not allocate by accident.
const MAX_LOCAL_SAMPLES: usize = 1 << 27;

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
    /// How deep demand streams may nest. A pull descends one Rust stack
    /// frame per level *on the audio thread*, so this is a hard ceiling the
    /// compiler enforces, not a runtime check the callback would pay for.
    /// Sixteen is well past any musical use -- sclang's own patterns rarely go
    /// past three -- and far short of anything a callback stack would notice.
    const MAX_DEMAND_DEPTH: u32 = 16;
    // Demand nesting depth per UGen, parallel to `ugens`.
    let mut demand_depth: Vec<u32> = Vec::with_capacity(spec.ugens.len());
    // Synth-private feedback channels: size the buffer and require each
    // channel's LocalIn to precede its LocalOut (the one-block-delay contract).
    let mut num_locals = 0usize;
    let mut localin_channels = std::collections::HashSet::new();
    // Spectral chains: each `FFT` opens one; its window size, hop and window
    // are recorded here and its slot index is `chains.len()` at that point.
    let mut chains: Vec<ChainShape> = Vec::new();
    // The synth's own buffers, one per `LocalBuf`: channels and frames.
    let mut local_buffers: Vec<(usize, usize)> = Vec::new();

    let (control_types, lagged) = parse_controls(&spec)?;

    for (i, u) in spec.ugens.iter().enumerate() {
        let desc =
            lookup(&u.kind).ok_or_else(|| format!("ugens[{i}]: unknown kind '{}'", u.kind))?;
        let fill_from = check_arity(i, u, desc)?;

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

        let mut config = ugen_config(i, u, desc)?;
        conv_size(i, u, desc, &mut config)?;

        // LocalBuf: its shape (inputs 0 and 1, scsynth's order) sizes memory
        // the instance allocates, so both must be constants.
        if desc.name == "LocalBuf" {
            let shape = |k: usize| match u.inputs.get(k) {
                Some(InputSpec::Const(x)) if x.is_finite() && *x >= 1.0 => Ok(*x as usize),
                None => Ok(1),
                _ => Err(format!(
                    "ugens[{i}] (LocalBuf): input {k} (its channels, then its frames) must be a \
                     constant of at least 1"
                )),
            };
            let (channels, frames) = (shape(0)?, shape(1)?);
            if channels.saturating_mul(frames) > MAX_LOCAL_SAMPLES {
                return Err(format!(
                    "ugens[{i}] (LocalBuf): {channels} x {frames} samples is past the limit of \
                     {MAX_LOCAL_SAMPLES}"
                ));
            }
            config.local_buffer = Some(local_buffers.len());
            local_buffers.push((channels, frames));
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
        // The optional tail the def stopped before, filled from the
        // descriptor's declared defaults. It happens here, on the network
        // thread, once per def: the compiled def is byte-identical to one a
        // complete client sent, so the audio thread never learns of it.
        if let Some(from) = fill_from {
            for slot in &desc.inputs[from..] {
                constants.push(slot.default);
                inputs.push(InputRef::Const(constants.len() - 1));
            }
        }

        let (chain_slot, chain_slot_b) =
            spectral_chain(i, u, desc, &inputs, &ugens, &mut chains, &mut config)?;

        // Output rate: the explicit `rate` field validated against the
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

        // Rate coercion. Lower rates widen into higher-rate inputs for
        // free, so the only illegal narrowings are:
        //  - an `ir` UGen with a non-`ir` input (it is computed once at init --
        //    a varying source cannot be frozen);
        //  - demand rate crossing the block boundary: a `dr` wire may only
        //    feed a demand driver's source slot, and that slot must be `dr`.
        for (k, r) in inputs.iter().enumerate() {
            let in_rate = match r {
                InputRef::Const(_) => Rate::Ir,
                // A scalar (`ir`) control is init-rate; every other control is
                // control-rate (pairs `ir` controls with the `ir` rate).
                InputRef::Control(c) if control_types[*c] == ControlType::Scalar => Rate::Ir,
                InputRef::Control(_) => Rate::Kr,
                InputRef::Wire(w) => ugens[*w].rate,
            };
            // A `dr` wire may only feed something that *pulls* it: a driver
            // (`Demand`/`Duty`/`TDuty`) or another demand source nesting it
            // (`Dseq(1, Dwhite(...), 3)`). Anywhere else it would cross
            // into block order, where a stream has no samples at all.
            let pulls_demand = desc.exec == ExecMode::DemandDriver || rate == Rate::Dr;
            if in_rate == Rate::Dr {
                if !pulls_demand {
                    return Err(format!(
                        "ugens[{i}] ({}).inputs[{k}]: a demand-rate (dr) signal can only feed a \
                         demand driver or another demand UGen",
                        u.kind
                    ));
                }
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
                        "ugens[{i}] ({}).inputs[0]: {} has no done flag -- only a UGen that \
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

        // Nesting depth of the demand sub-graph rooted here. A pull
        // recurses once per level, on the audio thread, so the depth is capped
        // here rather than guarded there: a def that nests too deep is a def we
        // refuse, not a stack the callback might run off. One more than the
        // deepest stream this UGen reads; 0 for a UGen that reads none.
        let depth = inputs
            .iter()
            .filter_map(|r| match r {
                InputRef::Wire(w) if ugens[*w].rate == Rate::Dr => Some(demand_depth[*w] + 1),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        if depth > MAX_DEMAND_DEPTH {
            return Err(format!(
                "ugens[{i}] ({}): demand streams nested {depth} deep; the limit is \
                 {MAX_DEMAND_DEPTH}",
                u.kind
            ));
        }
        demand_depth.push(depth);

        ugens.push(UGenDef {
            desc,
            inputs,
            rate,
            config,
            chain_slot,
            chain_slot_b,
        });
    }

    let ugens = insert_lags(ugens, &lagged, n_controls, &mut constants);

    Ok(SynthDef {
        name: spec.name,
        control_names: spec.controls.iter().map(|c| c.name.clone()).collect(),
        control_defaults: spec.controls.iter().map(|c| c.default).collect(),
        control_types,
        control_display: spec.controls.iter().map(|c| c.display.clone()).collect(),
        constants,
        ugens,
        num_locals,
        spectral_sizes: chains.iter().map(|c| c.winsize).collect(),
        local_buffers,
    })
}

/// The controls' types, and which are lagged: `(control index, up time,
/// optional down time)` for the compile-time `Lag` insertion.
#[allow(clippy::type_complexity)]
fn parse_controls(
    spec: &SynthDefSpec,
) -> Result<(Vec<ControlType>, Vec<(usize, f32, Option<f32>)>), String> {
    for control in &spec.controls {
        control.display.validate()?;
    }
    // Control types + lag times. `lagged` collects (control index, up
    // time, optional down time) for the compile-time Lag insertion below.
    let mut control_types = Vec::with_capacity(spec.controls.len());
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
    Ok((control_types, lagged))
}

/// A spectral chain's window size, and its hop and window as the `FFT` that
/// opened it gave them (`None` is the default). The resynthesis is exact only when the `IFFT`
/// overlap-adds at the analysis hop with the analysis window, so every UGen
/// on the chain takes these from the chain -- never from the wire it happens
/// to read, which for a two-chain combiner carries neither.
#[derive(Clone, Copy)]
struct ChainShape {
    winsize: usize,
    /// The frame the chain's first UGen windows (`winsize` on the wire),
    /// `None` for the whole transform.
    frame: Option<usize>,
    hop: Option<f32>,
    wintype: Option<i32>,
    /// Whether its frames are written (`FFTTrigger`) rather than analysed.
    written: bool,
}

impl ChainShape {
    /// The frame length in samples.
    fn frame_len(&self) -> usize {
        resolve_frame(self.winsize, self.frame)
    }
}

/// UGen `i`'s place in a spectral chain, as `(chain slot, second chain
/// slot)`. A `Source` (`FFT`) or `Trigger` (`FFTTrigger`) opens a chain and
/// records its window size, frame, hop and window; a `Copy` (`PV_Copy`)
/// opens one of its source's shape; a filter, sink or two-chain combiner
/// reads the chain its input carries and takes all of it from it.
fn spectral_chain(
    i: usize,
    u: &UGenSpec,
    desc: &UGenDescriptor,
    inputs: &[InputRef],
    ugens: &[UGenDef],
    chains: &mut Vec<ChainShape>,
    config: &mut UGenConfig,
) -> Result<(Option<usize>, Option<usize>), String> {
    // Spectral chain. A `Source`/`Trigger` opens a new chain: validate its
    // window size and record its slot. A `Filter`/`Sink` (`PV_*`/`IFFT`)
    // must take a spectral wire as input 0 and takes that chain's slot,
    // window size, frame, hop and window type -- so the client only
    // specifies them once, on the chain's first UGen. One given again
    // downstream must agree.
    let mut chain_slot: Option<usize> = None;
    let mut chain_slot_b: Option<usize> = None;
    // Resolves spectral input `k` to the chain slot it carries.
    let chain_of = |k: usize, inputs: &[InputRef], ugens: &[UGenDef]| -> Result<usize, String> {
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
        // A sink's output is audio or a value, not the chain it read.
        let carries = ugens[w].desc.spectral != SpectralRole::Sink;
        ugens[w].chain_slot.filter(|_| carries).ok_or_else(|| {
            format!(
                "ugens[{i}] ({}): input {k} (ugen {w}, {}) is not a spectral chain",
                u.kind, ugens[w].desc.name
            )
        })
    };
    match desc.spectral {
        SpectralRole::None => {}
        SpectralRole::Source | SpectralRole::Trigger => {
            let winsize = resolve_fft_size(u.fft_size);
            config.fft_size = Some(winsize);
            if let Some(w) = u.winsize
                && !(MIN_HOP..=winsize).contains(&w)
            {
                return Err(format!(
                    "ugens[{i}] ({}): winsize {w} is out of range; it must be at least \
                     {MIN_HOP} samples and at most the fft_size ({winsize})",
                    u.kind
                ));
            }
            let frame = resolve_frame(winsize, u.winsize);
            // A hop is a fraction of the frame in (0, 1], at least one block
            // long: shorter would close twice in one slice, longer would leave
            // gaps no frame covers. Rejected rather than moved into range, so
            // a chain never runs at a hop other than the one it was given.
            if let Some(hop) = config.hop {
                let samples = (frame as f32 * hop).round();
                if !(hop > 0.0 && hop <= 1.0) || samples < MIN_HOP as f32 {
                    return Err(format!(
                        "ugens[{i}] ({}): hop {hop} of a {frame}-sample frame is out \
                         of range; it must be in (0, 1] and at least {MIN_HOP} samples \
                         ({:.4} here)",
                        u.kind,
                        MIN_HOP as f32 / frame as f32
                    ));
                }
            }
            let written = desc.spectral == SpectralRole::Trigger;
            config.written = written;
            chain_slot = Some(chains.len());
            chains.push(ChainShape {
                winsize,
                frame: u.winsize,
                hop: config.hop,
                wintype: config.wintype,
                written,
            });
        }
        SpectralRole::Filter | SpectralRole::Sink => {
            let slot = chain_of(0, inputs, ugens)?;
            chain_slot = Some(slot);
            take_chain_shape(i, u, chains[slot], config)?;
        }
        // A copy opens a chain of its source's shape; it runs as a pair,
        // the copy as chain A, the source as chain B.
        SpectralRole::Copy => {
            let src = chain_of(0, inputs, ugens)?;
            let shape = chains[src];
            take_chain_shape(i, u, shape, config)?;
            chain_slot = Some(chains.len());
            chain_slot_b = Some(src);
            chains.push(shape);
        }
        // A two-chain combiner: inputs 0 and 1 are chains of equal
        // window size and distinct slots; the result lands in chain A, so
        // the combiner inherits A's slot (a downstream filter/sink then
        // reads the combined chain through it).
        SpectralRole::Filter2 => {
            let a = chain_of(0, inputs, ugens)?;
            let b = chain_of(1, inputs, ugens)?;
            if a == b {
                return Err(format!(
                    "ugens[{i}] ({}): both inputs read the same spectral chain",
                    u.kind
                ));
            }
            let (sa, sb) = (chains[a], chains[b]);
            if sa.winsize != sb.winsize {
                return Err(format!(
                    "ugens[{i}] ({}): chain window sizes differ ({} vs {})",
                    u.kind, sa.winsize, sb.winsize
                ));
            }
            if sa.frame_len() != sb.frame_len() {
                return Err(format!(
                    "ugens[{i}] ({}): chain frames differ ({} vs {} samples)",
                    u.kind,
                    sa.frame_len(),
                    sb.frame_len()
                ));
            }
            // The two frames are combined bin by bin and resynthesized as
            // chain A's: B has to be analysed at the same cadence and with the
            // same window, or the overlap-add scales and spaces its share
            // wrongly.
            let frame = sa.frame_len();
            if resolve_hop(frame, sa.hop) != resolve_hop(frame, sb.hop) {
                return Err(format!(
                    "ugens[{i}] ({}): chain hops differ ({} vs {} samples)",
                    u.kind,
                    resolve_hop(frame, sa.hop),
                    resolve_hop(frame, sb.hop)
                ));
            }
            if window_of(sa.wintype) != window_of(sb.wintype) {
                return Err(format!(
                    "ugens[{i}] ({}): chain windows differ ({:?} vs {:?})",
                    u.kind,
                    window_of(sa.wintype),
                    window_of(sb.wintype)
                ));
            }
            chain_slot = Some(a);
            chain_slot_b = Some(b);
            take_chain_shape(i, u, sa, config)?;
        }
    }
    Ok((chain_slot, chain_slot_b))
}

/// The convolution rows whose size is an input, as scsynth's
/// constructors read it: which input, and whether the transform is twice it
/// (a `framesize`) or it (an `fftsize`). `Convolution3`'s is its kernel
/// length, in the time domain.
fn conv_size_input(kind: &str) -> Option<(usize, Option<bool>)> {
    match kind {
        "PartConv" => Some((1, Some(false))),
        "Convolution" => Some((2, Some(true))),
        "Convolution2" | "Convolution2L" => Some((3, Some(true))),
        "StereoConvolution2L" => Some((4, Some(true))),
        "Convolution3" => Some((3, None)),
        _ => None,
    }
}

/// The longest kernel `Convolution3` takes, in samples: it sums the whole
/// kernel at every sample, so a longer one is a partitioned convolution's.
const MAX_DIRECT_KERNEL: usize = 65536;

/// Resolves a convolution row's size input into its `fft_size`: a constant,
/// since it sizes memory, and a transform size the FFT supports.
fn conv_size(
    i: usize,
    u: &UGenSpec,
    desc: &UGenDescriptor,
    config: &mut UGenConfig,
) -> Result<(), String> {
    let Some((k, doubled)) = conv_size_input(desc.name) else {
        return Ok(());
    };
    let name = desc.inputs[k].name;
    let size = match u.inputs.get(k) {
        Some(InputSpec::Const(x)) => *x,
        None => desc.inputs[k].default,
        Some(_) => {
            return Err(format!(
                "ugens[{i}] ({}): {name} (input {k}) sizes its memory, so it must be a constant",
                u.kind
            ));
        }
    };
    let whole = size.is_finite() && size >= 1.0 && size.fract() == 0.0;
    match doubled {
        None => {
            if !whole || size as usize > MAX_DIRECT_KERNEL {
                return Err(format!(
                    "ugens[{i}] ({}): {name} {size} must be a whole number of samples from 1 \
                     to {MAX_DIRECT_KERNEL}",
                    u.kind
                ));
            }
        }
        Some(doubled) => {
            let n = if doubled { 2.0 * size } else { size };
            if !whole || !fft::supports(n as usize) {
                let sizes: Vec<usize> = fft::SUPPORTED_SIZES
                    .iter()
                    .map(|&s| if doubled { s / 2 } else { s })
                    .collect();
                return Err(format!(
                    "ugens[{i}] ({}): unsupported {name} {size}; use one of {sizes:?}",
                    u.kind
                ));
            }
            config.fft_size = Some(n as usize);
        }
    }
    Ok(())
}

/// The window a def's `wintype` names, with the default for an unset one.
fn window_of(wintype: Option<i32>) -> Window {
    Window::from_wintype(wintype.unwrap_or(0))
}

/// Gives a UGen on a chain the chain's size, hop and window. A hop or window
/// the def repeats on the UGen must be the chain's: a synthesis window or hop
/// other than the analysis one resynthesizes at the wrong level.
fn take_chain_shape(
    i: usize,
    u: &UGenSpec,
    shape: ChainShape,
    config: &mut UGenConfig,
) -> Result<(), String> {
    let frame = shape.frame_len();
    if let Some(hop) = config.hop
        && resolve_hop(frame, Some(hop)) != resolve_hop(frame, shape.hop)
    {
        return Err(format!(
            "ugens[{i}] ({}): hop {hop} differs from its chain's FFT ({} samples); \
             give it only on the FFT",
            u.kind,
            resolve_hop(frame, shape.hop)
        ));
    }
    if config.wintype.is_some() && window_of(config.wintype) != window_of(shape.wintype) {
        return Err(format!(
            "ugens[{i}] ({}): window {:?} differs from its chain's FFT ({:?}); \
             give it only on the FFT",
            u.kind,
            window_of(config.wintype),
            window_of(shape.wintype)
        ));
    }
    if let Some(w) = config.winsize
        && w != frame
    {
        return Err(format!(
            "ugens[{i}] ({}): winsize {w} differs from its chain's FFT ({frame}); \
             give it only on the FFT",
            u.kind
        ));
    }
    config.fft_size = Some(shape.winsize);
    config.winsize = shape.frame;
    config.hop = shape.hop;
    config.wintype = shape.wintype;
    config.written = shape.written;
    Ok(())
}

/// UGen `i`'s arity against its kind's. `Some(n)` when the def stopped `n`
/// inputs in, inside the kind's declared optional tail, which the caller
/// fills from the descriptor's defaults.
fn check_arity(i: usize, u: &UGenSpec, desc: &UGenDescriptor) -> Result<Option<usize>, String> {
    // Arity, and the one way a def may be short: the kind's **declared
    // optional tail** (`UGenInput::optional`), which the fill below
    // completes from the descriptor's defaults. A kind with no tail --
    // which is most of them, and every operator -- still needs its inputs
    // exactly, so a truncated `Mul` fails here rather than compiling to
    // silence.
    let mut fill_from = None;
    if let Arity::Fixed(want) = desc.arity
        && u.inputs.len() != want
    {
        let least = desc.required_inputs();
        if u.inputs.len() > want || u.inputs.len() < least {
            return Err(if least == want {
                format!(
                    "ugens[{i}] ({}): expected {want} inputs, got {}",
                    u.kind,
                    u.inputs.len()
                )
            } else {
                format!(
                    "ugens[{i}] ({}): expected {least} to {want} inputs, got {}",
                    u.kind,
                    u.inputs.len()
                )
            });
        }
        fill_from = Some(u.inputs.len());
    }
    if u.inputs.len() > MAX_UGEN_INPUTS {
        return Err(format!(
            "ugens[{i}] ({}): inputs ({}) exceed MAX_UGEN_INPUTS ({MAX_UGEN_INPUTS})",
            u.kind,
            u.inputs.len()
        ));
    }
    Ok(fill_from)
}

/// UGen `i`'s static configuration: its path, its operator and clip resolved
/// by name, its transform size checked, and its `PV_Kernel` programs
/// compiled -- everything about it that is not a wire.
fn ugen_config(i: usize, u: &UGenSpec, desc: &UGenDescriptor) -> Result<UGenConfig, String> {
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
        let name =
            u.op.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| {
                format!("ugens[{i}] ({}): requires an 'op' operator name", u.kind)
            })?;
        let resolved = match family {
            OpFamily::Unary => builtins::UnaryOp::from_name(name).map(|o| o as u32),
            OpFamily::Binary => builtins::BinaryOp::from_name(name).map(|o| o as u32),
            OpFamily::Map => clausters_core::warp::MapOp::from_name(name).map(|o| o as u32),
            OpFamily::Draw => clausters_core::rng::Draw::from_name(name).map(|o| o as u32),
        };
        op_index = Some(resolved.ok_or_else(|| {
            format!(
                "ugens[{i}] ({}): unknown {family:?} operator '{name}'",
                u.kind
            )
        })?);
    }
    // The clip is `RangeMapUGen`'s other static field: a name, resolved
    // here so a typo fails the def rather than silently trimming nothing.
    let clip = match u.clip.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(name) => Some(
            clausters_core::warp::Clip::from_name(name)
                .ok_or_else(|| format!("ugens[{i}] ({}): unknown clip '{name}'", u.kind))?
                as u32,
        ),
    };
    let mut config = UGenConfig {
        path: u.path.clone(),
        looping: u.looping,
        start: u.start,
        format: u.format.clone(),
        op: op_index,
        clip,
        label: u.label.clone(),
        fft_size: u.fft_size,
        hop: u.hop,
        wintype: u.wintype,
        winsize: u.winsize,
        written: false,
        partitions: u.partitions,
        mag_prog: None,
        phase_prog: None,
        max_delay: u.max_delay,
        inputs: u.inputs.len(),
        constants: u
            .inputs
            .iter()
            .map(|i| match i {
                InputSpec::Const(v) => Some(*v),
                _ => None,
            })
            .collect(),
        local_buffer: None,
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
    // `PV_Kernel` bin expressions: resolve and validate the postfix
    // token lists now, so the RT thread only ever runs a program that
    // passed the stack/arity checks. The parameters a program may read
    // (`p0`...) are this UGen's inputs past the chain (input 0).
    let n_params = u.inputs.len().saturating_sub(1);
    if let Some(tokens) = &u.mag_expr {
        let what = format!("ugens[{i}] ({}) mag_expr", u.kind);
        config.mag_prog = Some(compile_pv_expr(tokens, n_params, &what)?);
    }
    if let Some(tokens) = &u.phase_expr {
        let what = format!("ugens[{i}] ({}) phase_expr", u.kind);
        config.phase_prog = Some(compile_pv_expr(tokens, n_params, &what)?);
    }
    Ok(config)
}

/// Compile-time lag insertion, over the compiled `ugens`.
fn insert_lags(
    mut ugens: Vec<UGenDef>,
    lagged: &[(usize, f32, Option<f32>)],
    n_controls: usize,
    constants: &mut Vec<f32>,
) -> Vec<UGenDef> {
    // Compile-time lag insertion: a lagged control compiles to a `Lag`
    // (or `LagUD`) UGen reading the raw control, prepended to the graph; every
    // reference to that control is rewritten to the smoother's output. Reusing
    // the real UGen keeps a single lag implementation shared with the library
    // (no bespoke control path). The smoothers run at audio rate, so a lagged
    // control glides per sample toward its block-constant target.
    if !lagged.is_empty() {
        let n_lag = lagged.len();
        let lag_desc = lookup("Lag").expect("Lag is registered");
        let lagud_desc = lookup("LagUD").expect("LagUD is registered");
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
                    (lagud_desc, vec![InputRef::Control(ci), up_ref, down_ref])
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
    ugens
}

/// The built-in "default" def, registered at startup: a subtractive voice
/// with a control for each dimension an MPE zone plays, so a note from a
/// pattern, a MIDI channel or a zone sounds with nothing sent first.
/// Built through the same spec/compile path as client-sent defs: the graph is
/// `default.json`, the wire format verbatim.
///
/// The tone is two sawtooths detuned by +/-0.4 %, through a resonant lowpass
/// (`RLPF`, `rq` 0.8) whose cutoff follows the key -- half an octave above it
/// at rest -- and opens with:
///
/// - `slide` (0-1, 0.5 unless set; MPE's third dimension, CC 74): up to
///   two and a half octaves more (so 1.75 above the key when centred);
/// - `press` (0-1; pressure): up to two octaves more, and half again the
///   level;
/// - `amp` (the velocity): a **bloom** -- the cutoff opens at the attack and
///   closes over 0.6 s, by more the harder the note.
///
/// The cutoff stops at 16 kHz. `press` and `slide` are smoothed over 50 ms,
/// so a controller moves the tone without stepping. `pan` places the voice
/// at equal power (`Pan2`, -1 left to 1 right) on the bus `out` names and the
/// one after it -- buses 0 and 1, the hardware's first two, unless set: the
/// control an event's `out` key writes, and the one a box of notes on a
/// track sets to reach that track's strip.
///
/// The level follows a gated ASR (equal-power sine ramps: 0.01 s attack,
/// sustain at 1.0 while the gate is held, 0.3 s release) with
/// `doneAction = FREE_SELF`, so the note ramps in and out without a click and
/// frees itself once the release finishes. A rising `gate` (re)triggers; a
/// `gate 0` starts the release. The client releases this instrument via the
/// gate (see the Python player's `play_event`), which is what lets the
/// release ring out.
pub fn default_spec() -> SynthDefSpec {
    serde_json::from_str(include_str!("default.json"))
        .expect("the built-in default def is valid JSON")
}
