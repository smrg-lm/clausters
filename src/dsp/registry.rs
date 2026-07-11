//! The UGen catalog as **data**: one [`UGenDescriptor`] per kind, holding
//! everything the compiler and engine need (name, arity, rates, bus role,
//! execution mode, constructor). There is no central `match kind { … }`: the
//! compiler and the bus analysis read descriptor fields, so they stay generic
//! and **adding a UGen is a single entry in `UGENS`** — the catalog grows
//! without touching general logic.
//!
//! The small closed sets that *are* general logic stay as enums: [`Rate`]
//! (the four calculation rates), [`ExecMode`] (how the synth runs a UGen that
//! needs cross-ugen coordination), [`BusRole`] (its audio-bus role for the
//! M12 dependency analysis) and [`Arity`]. A UGen names one of each; it does
//! not invent new control flow.

use crate::dsp::binop::{BinOp, BinaryOp};
use crate::dsp::buf::{BufInfo, BufInfoKind, BufRd, PlayBuf};
use crate::dsp::demand::{Demand, Dseq};
use crate::dsp::disk::{DiskIn, DiskOut};
use crate::dsp::envgen::EnvGen;
use crate::dsp::fused::{MulAdd, Sum3, Sum4};
use crate::dsp::impulse::Impulse;
use crate::dsp::io::{In, InCtl, Out, OutCtl, ReplaceOut};
use crate::dsp::lag::{Lag, VarLag};
use crate::dsp::local::{LocalIn, LocalOut};
use crate::dsp::noise::WhiteNoise;
use crate::dsp::osc::{Osc, OscN, Shaper, VOsc};
use crate::dsp::reply::{Poll, SendReply, SendTrig};
use crate::dsp::scalar::{Rand, SampleRate};
use crate::dsp::sinosc::SinOsc;
use crate::dsp::spectral::{Fft, Ifft, MagMode, PvBrickWall, PvMag};
use crate::dsp::unop::UnaryOp;
use crate::dsp::{Rate, UGen};

/// Input slot of a demand driver ([`ExecMode::DemandDriver`]) that names its
/// demand source (after `trig`, `reset`): must be a wire to a demand-rate
/// (`dr`) UGen.
pub const DEMAND_SOURCE_SLOT: usize = 2;

/// Static, per-UGen parameters that are not signal inputs: set in the SynthDef
/// spec and resolved at compile time, consumed by a descriptor's `build`.
/// Empty for almost every UGen; `DiskIn`/`DiskOut` use it to carry their file
/// path and options.
#[derive(Clone, Debug, Default)]
pub struct UGenConfig {
    /// File path for `DiskIn`/`DiskOut`.
    pub path: Option<String>,
    /// `DiskIn`: restart from the top of the file at end of stream.
    pub looping: bool,
    /// `DiskOut` WAV sample format (`int16` | `int24` | `float`).
    pub format: Option<String>,
    /// Special-index operator for `BinaryOpUGen`/`UnaryOpUGen` — a
    /// `clausters_core::builtins` opcode discriminant, validated at compile
    /// time and read by their `build`.
    pub op: Option<u32>,
    /// Static string tag for the side-effect UGens (S9): `SendReply`'s command
    /// name (the OSC address it replies with, default `/reply`) and `Poll`'s
    /// label (default `poll`). Ignored by every other kind.
    pub label: Option<String>,
    /// Spectral chain (S8): FFT window size, a power of two (`FFT`/`IFFT`/
    /// `PV_*`). Sizes the pre-allocated transform scratch, so it is static
    /// config, not a signal input. The compiler propagates the `FFT`'s size to
    /// the rest of its chain. Ignored by every other kind.
    pub fft_size: Option<usize>,
    /// Spectral chain (S8): hop as a fraction of the window (`FFT`), default
    /// `0.5`. Ignored by every other kind.
    pub hop: Option<f32>,
    /// Spectral chain (S8): window type (`FFT`/`IFFT`), a
    /// [`Window`](clausters_core::window::Window) `wintype` integer, default `0`
    /// (Hann). Also settable live via `/u_cmd`. Ignored by every other kind.
    pub wintype: Option<i32>,
}

/// Input count of a UGen: a fixed number, or variable (`EnvGen`, `Dseq`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arity {
    Fixed(usize),
    Variadic,
}

/// How the synth runs a UGen that needs coordination the plain `process` path
/// cannot express — state shared across the whole ugen vector. Everything else
/// is [`ExecMode::Normal`] and runs through [`UGen::process`]. This is
/// the *only* per-UGen behavior the engine special-cases, and it is a small
/// closed set (not a per-kind switch): see `synthdef::instance`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecMode {
    /// Runs through `UGen::process` like the vast majority of UGens.
    Normal,
    /// Reads a synth-private feedback channel (`LocalIn`).
    LocalIn,
    /// Writes a synth-private feedback channel and passes through (`LocalOut`).
    LocalOut,
    /// Pulls its demand source each block (`Demand`); see the `dr` contract.
    DemandDriver,
    /// Runs through [`UGen::process_spectral`] with its synth-private
    /// [`SpectralChain`](crate::dsp::spectral::SpectralChain) (`FFT`/`PV_*`/
    /// `IFFT`, S8). The `spectral` role field says how it uses the chain.
    Spectral,
}

/// A spectral-chain UGen's place in the `FFT`→`PV_*`→`IFFT` pipeline (S8), used
/// by the compiler to allocate and thread the synth-private
/// [`SpectralChain`](crate::dsp::spectral::SpectralChain). `None` on every
/// non-spectral kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectralRole {
    /// Not a spectral UGen.
    None,
    /// Opens a chain: analyses audio into a fresh chain (`FFT`). Gets its own
    /// new chain slot.
    Source,
    /// Transforms a chain in place (`PV_*`). Its input 0 is the upstream chain
    /// wire; it inherits that chain's slot.
    Filter,
    /// Closes a chain: resynthesises audio from it (`IFFT`). Its input 0 is the
    /// upstream chain wire; it inherits that chain's slot.
    Sink,
}

/// The operator family of a generic op UGen (`BinaryOpUGen`/`UnaryOpUGen`):
/// which `clausters_core::builtins` opcode table its `op` index selects. The
/// compiler uses it to validate `op` before instantiation. `None` on every
/// other kind (whose behavior is fixed by its name, not an index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpFamily {
    Unary,
    Binary,
}

/// The audio-bus role a UGen plays, for the M12 dependency analysis
/// (`osc::graph::ugen_usage`), read off input 0 (the bus index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusRole {
    /// Touches no audio bus (the default).
    None,
    /// Reads the bus (`In`).
    Read,
    /// Writes the bus (`Out`).
    Write,
    /// Reads and writes the bus (`ReplaceOut` consumes what it overwrites).
    ReadWrite,
}

/// Everything the compiler and engine need to know about one UGen kind, as
/// data co-located per UGen. Adding a UGen is one `UGENS` entry; nothing
/// else changes.
pub struct UGenDescriptor {
    /// Wire name (the def's `"kind"`).
    pub name: &'static str,
    pub arity: Arity,
    /// Rate when the def omits `"rate"`.
    pub default_rate: Rate,
    /// Rates this UGen may be instantiated at (the compiler rejects the rest).
    pub rates: &'static [Rate],
    pub exec: ExecMode,
    pub bus: BusRole,
    /// Requires a non-empty `path` in the config (`DiskIn`/`DiskOut`).
    pub needs_path: bool,
    /// Generic op UGen: requires a valid `op` index of this family in the
    /// config (`BinaryOpUGen`/`UnaryOpUGen`). `None` for every fixed-behavior
    /// kind.
    pub op_family: Option<OpFamily>,
    /// Spectral-chain role (S8): whether this kind opens, transforms or closes
    /// an `FFT` chain. [`SpectralRole::None`] for every non-spectral kind.
    pub spectral: SpectralRole,
    /// Builds an instance. Runs on the network thread (allocates); `config`
    /// carries static per-UGen parameters, ignored by most kinds.
    pub build: fn(&UGenConfig) -> Box<dyn UGen>,
}

impl UGenDescriptor {
    /// Whether this kind may be instantiated at `rate`.
    pub fn allows(&self, rate: Rate) -> bool {
        self.rates.contains(&rate)
    }
}

// Rate sets, shared by the table. The default (a plain signal processor) is
// audio-or-control rate; the exceptions widen (`ir`/`dr` scalars and the
// demand source) or narrow to audio-only (whole-block I/O).
const R_KR_AR: &[Rate] = &[Rate::Kr, Rate::Ar];
const R_KR: &[Rate] = &[Rate::Kr];
const R_AR: &[Rate] = &[Rate::Ar];
const R_IR_KR: &[Rate] = &[Rate::Ir, Rate::Kr];
const R_IR_KR_AR: &[Rate] = &[Rate::Ir, Rate::Kr, Rate::Ar];
const R_IR: &[Rate] = &[Rate::Ir];
const R_DR: &[Rate] = &[Rate::Dr];

/// Full descriptor constructor.
#[allow(clippy::too_many_arguments)]
const fn desc_full(
    name: &'static str,
    arity: Arity,
    default_rate: Rate,
    rates: &'static [Rate],
    exec: ExecMode,
    bus: BusRole,
    needs_path: bool,
    op_family: Option<OpFamily>,
    spectral: SpectralRole,
    build: fn(&UGenConfig) -> Box<dyn UGen>,
) -> UGenDescriptor {
    UGenDescriptor {
        name,
        arity,
        default_rate,
        rates,
        exec,
        bus,
        needs_path,
        op_family,
        spectral,
        build,
    }
}

/// Compact descriptor constructor (keeps `UGENS` readable as a table): a plain
/// fixed-behavior kind, no `op` family.
#[allow(clippy::too_many_arguments)]
const fn desc(
    name: &'static str,
    arity: Arity,
    default_rate: Rate,
    rates: &'static [Rate],
    exec: ExecMode,
    bus: BusRole,
    needs_path: bool,
    build: fn(&UGenConfig) -> Box<dyn UGen>,
) -> UGenDescriptor {
    desc_full(
        name,
        arity,
        default_rate,
        rates,
        exec,
        bus,
        needs_path,
        None,
        SpectralRole::None,
        build,
    )
}

/// Descriptor for a spectral-chain UGen (`FFT`/`PV_*`/`IFFT`, S8): it runs
/// through [`UGen::process_spectral`] on the synth-private chain. `FFT` and the
/// `PV_*` filters carry the chain at control rate (one marker per block); `IFFT`
/// produces audio.
const fn desc_spectral(
    name: &'static str,
    arity: Arity,
    default_rate: Rate,
    rates: &'static [Rate],
    role: SpectralRole,
    build: fn(&UGenConfig) -> Box<dyn UGen>,
) -> UGenDescriptor {
    desc_full(
        name,
        arity,
        default_rate,
        rates,
        ExecMode::Spectral,
        BusRole::None,
        false,
        None,
        role,
        build,
    )
}

/// Descriptor for a generic op UGen (`BinaryOpUGen`/`UnaryOpUGen`): audio-or-
/// control rate, no bus/path, its behavior chosen by the config `op` index.
const fn desc_op(
    name: &'static str,
    arity: Arity,
    family: OpFamily,
    build: fn(&UGenConfig) -> Box<dyn UGen>,
) -> UGenDescriptor {
    desc_full(
        name,
        arity,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        Some(family),
        SpectralRole::None,
        build,
    )
}

use Arity::{Fixed, Variadic};
use BusRole::{Read, ReadWrite, Write};
use ExecMode::{DemandDriver, LocalIn as ExecLocalIn, LocalOut as ExecLocalOut, Normal};
use Rate::{Ar, Dr, Ir, Kr};

/// The UGen catalog. **To add a UGen, add one row here** (plus its `dsp`
/// module) — the compiler and bus analysis pick it up with no other change.
static UGENS: &[UGenDescriptor] = &[
    // --- generators (audio or control rate; the default shape) ---
    desc(
        "SinOsc",
        Fixed(1),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(SinOsc::new()),
    ),
    desc(
        "Impulse",
        Fixed(1),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(Impulse::new()),
    ),
    desc(
        "WhiteNoise",
        Fixed(0),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(WhiteNoise::new()),
    ),
    // --- arithmetic: the generic op UGens (S3), selected by a core opcode
    //     index; every math need is one more `clausters_core::builtins` entry,
    //     not a new kind. `Add`/`Sub`/`Mul`/`Div` stay as thin aliases below
    //     for back-compat with existing defs. ---
    desc_op("BinaryOpUGen", Fixed(2), OpFamily::Binary, |c| {
        Box::new(BinaryOp::from_index(c.op.unwrap_or(0)))
    }),
    desc_op("UnaryOpUGen", Fixed(1), OpFamily::Unary, |c| {
        Box::new(UnaryOp::from_index(c.op.unwrap_or(0)))
    }),
    // Fused forms scsynth optimizes (fixed kinds, not op-table entries).
    desc(
        "MulAdd",
        Fixed(3),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(MulAdd),
    ),
    desc(
        "Sum3",
        Fixed(3),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(Sum3),
    ),
    desc(
        "Sum4",
        Fixed(4),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(Sum4),
    ),
    // Aliases for the four operator kinds (back-compat).
    desc(
        "Add",
        Fixed(2),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(BinaryOp::new(BinOp::Add)),
    ),
    desc(
        "Sub",
        Fixed(2),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(BinaryOp::new(BinOp::Sub)),
    ),
    desc(
        "Mul",
        Fixed(2),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(BinaryOp::new(BinOp::Mul)),
    ),
    desc(
        "Div",
        Fixed(2),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(BinaryOp::new(BinOp::Div)),
    ),
    // --- audio-bus I/O (audio rate only; carries a bus role) ---
    desc("In", Fixed(1), Ar, R_AR, Normal, Read, false, |_| {
        Box::new(In)
    }),
    desc(
        "InCtl",
        Fixed(1),
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(InCtl),
    ),
    desc(
        "OutCtl",
        Fixed(2),
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(OutCtl),
    ),
    desc("Out", Fixed(2), Ar, R_AR, Normal, Write, false, |_| {
        Box::new(Out)
    }),
    desc(
        "ReplaceOut",
        Fixed(2),
        Ar,
        R_AR,
        Normal,
        ReadWrite,
        false,
        |_| Box::new(ReplaceOut),
    ),
    // --- buffer readers and info ---
    desc(
        "PlayBuf",
        Fixed(4),
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(PlayBuf::new()),
    ),
    desc(
        "BufRd",
        Fixed(4),
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(BufRd),
    ),
    // --- table oscillators & waveshaper (S5); read `/b_gen` wavetables ---
    desc(
        "Osc",
        Fixed(3),
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(Osc::new()),
    ),
    desc(
        "OscN",
        Fixed(3),
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(OscN::new()),
    ),
    desc(
        "VOsc",
        Fixed(3),
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(VOsc::new()),
    ),
    desc(
        "Shaper",
        Fixed(2),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(Shaper),
    ),
    desc(
        "BufSampleRate",
        Fixed(1),
        Ar,
        R_IR_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(BufInfo(BufInfoKind::SampleRate)),
    ),
    desc(
        "BufRateScale",
        Fixed(1),
        Ar,
        R_IR_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(BufInfo(BufInfoKind::RateScale)),
    ),
    desc(
        "BufFrames",
        Fixed(1),
        Ar,
        R_IR_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(BufInfo(BufInfoKind::Frames)),
    ),
    desc(
        "BufChannels",
        Fixed(1),
        Ar,
        R_IR_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(BufInfo(BufInfoKind::Channels)),
    ),
    desc(
        "BufDur",
        Fixed(1),
        Ar,
        R_IR_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(BufInfo(BufInfoKind::Duration)),
    ),
    // --- streaming disk I/O (need a path) ---
    desc(
        "DiskIn",
        Fixed(1),
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        true,
        |c| Box::new(DiskIn::open(c)),
    ),
    desc(
        "DiskOut",
        Fixed(1),
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        true,
        |c| Box::new(DiskOut::open(c)),
    ),
    // --- synth-private feedback (synth-coordinated execution) ---
    desc(
        "LocalIn",
        Fixed(1),
        Ar,
        R_AR,
        ExecLocalIn,
        BusRole::None,
        false,
        |_| Box::new(LocalIn),
    ),
    desc(
        "LocalOut",
        Fixed(2),
        Ar,
        R_AR,
        ExecLocalOut,
        BusRole::None,
        false,
        |_| Box::new(LocalOut),
    ),
    // --- one-pole smoothers (also inserted by S2 lagged controls) ---
    desc(
        "Lag",
        Fixed(2),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(Lag::new()),
    ),
    desc(
        "VarLag",
        Fixed(3),
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(VarLag::new()),
    ),
    // --- envelope ---
    desc(
        "EnvGen",
        Variadic,
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(EnvGen::new()),
    ),
    // --- scalar / init-rate (S1) ---
    desc(
        "SampleRate",
        Fixed(0),
        Ir,
        R_IR_KR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(SampleRate),
    ),
    desc(
        "Rand",
        Fixed(2),
        Ir,
        R_IR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(Rand::new()),
    ),
    // --- demand rate (S1): the driver runs specially, the source is pulled ---
    desc(
        "Demand",
        Fixed(3),
        Ar,
        R_KR_AR,
        DemandDriver,
        BusRole::None,
        false,
        |_| Box::new(Demand::new()),
    ),
    desc(
        "Dseq",
        Variadic,
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(Dseq::new()),
    ),
    // --- side-effect UGens (S9): reply/observe, no `Out` required. Control or
    //     audio rate; their output is silence (SendTrig/SendReply) or the
    //     polled signal passed through (Poll). ---
    desc(
        "SendTrig",
        Fixed(3),
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_| Box::new(SendTrig::new()),
    ),
    desc(
        "SendReply",
        Variadic,
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c| Box::new(SendReply::new(c)),
    ),
    desc(
        "Poll",
        Fixed(3),
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c| Box::new(Poll::new(c)),
    ),
    // --- frequency-domain (`fr`) chain (S8): FFT opens a synth-private
    //     spectral chain, PV_* transform it in place, IFFT resynthesises audio.
    //     FFT/PV carry the chain at control rate (a per-block ready marker);
    //     IFFT produces audio. See `dsp::spectral`. ---
    desc_spectral("FFT", Fixed(2), Kr, R_KR, SpectralRole::Source, |c| {
        Box::new(Fft::new(c))
    }),
    desc_spectral("IFFT", Fixed(1), Ar, R_AR, SpectralRole::Sink, |c| {
        Box::new(Ifft::new(c))
    }),
    desc_spectral(
        "PV_MagAbove",
        Fixed(2),
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_| Box::new(PvMag::new(MagMode::Above)),
    ),
    desc_spectral(
        "PV_MagBelow",
        Fixed(2),
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_| Box::new(PvMag::new(MagMode::Below)),
    ),
    desc_spectral(
        "PV_BrickWall",
        Fixed(2),
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_| Box::new(PvBrickWall),
    ),
];

/// Looks a UGen up by its wire name. Returns the descriptor (the single source
/// of truth for that kind) or `None` for an unknown name.
pub fn lookup(name: &str) -> Option<&'static UGenDescriptor> {
    UGENS.iter().find(|d| d.name == name)
}
