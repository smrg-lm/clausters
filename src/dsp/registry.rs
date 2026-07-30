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

use std::cell::Cell;

use clausters_core::rng::SEED_STRIDE;

use crate::dsp::binop::{BinOp, BinaryOp};
use crate::dsp::buf::{BufInfo, BufInfoKind, BufRd, PlayBuf};
use crate::dsp::conv::Conv;
use crate::dsp::delay::{Delay, Feedback, Interp};
use crate::dsp::demand::{
    Dbufrd, Demand, Dlist, Dramp, Drandom, Dstutter, Dswitch1, Duty, DutyKind, ListOrder, RampKind,
    RandKind,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::dsp::disk::{DiskIn, DiskOut};
use crate::dsp::envgen::EnvGen;
use crate::dsp::filter::{OneFilter, OneKind, Svf, SvfMode};
use crate::dsp::fused::{MulAdd, Sum3, Sum4};
use crate::dsp::impulse::Impulse;
use crate::dsp::io::{In, InCtl, Out, OutCtl, ReplaceOut};
use crate::dsp::lag::{Lag, VarLag};
use crate::dsp::line::{Line, LineShape};
use crate::dsp::local::{LocalIn, LocalOut};
use crate::dsp::nodectl::{SelfControl, WhenDone, WhenDoneMode};
use crate::dsp::noise::{
    BrownNoise, ClipNoise, Crackle, Dust, DustMode, GrayNoise, LfNoise, LfNoiseShape, PinkNoise,
    WhiteNoise,
};
use crate::dsp::osc::{Osc, OscN, Shaper, VOsc};
use crate::dsp::pan::{Pan, PanAz, PanKind, RotKind, Rotate, Select, SelectKind};
use crate::dsp::phase::{Lf, LfShape, Phasor, Pulse, Saw};
use crate::dsp::reply::{Poll, SendReply, SendTrig};
use crate::dsp::scalar::{Rand, SampleRate};
use crate::dsp::sine::Sine;
use crate::dsp::spectral::{
    CombineOp, Fft, Ifft, MagMode, PvBinShift, PvBrickWall, PvCombine, PvKernel, PvMag,
    PvMagFreeze, PvMagSmear,
};
use crate::dsp::trig::{
    Changed, Counter, CounterMode, Decay, DetectSilence, Elapsed, ElapsedMode, FlipFlop,
    FlipFlopMode, Hold, HoldMode, Schmidt, Stepper, TrigMode, TrigPulse,
};
use crate::dsp::unop::UnaryOp;
use crate::dsp::{DoneAction, Rate, UGen};

/// Line length a delay row allocates when the def omits `max_delay` (U3), in
/// seconds. A default rather than a hard error, like `fft_size`'s — but a def
/// that wants a long echo must say so, because the field sizes memory and
/// cannot be widened later.
const DEFAULT_MAX_DELAY: f32 = 0.2;

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
    /// `Conv` (M28): FDL capacity in partitions — the longest prepared kernel
    /// this instance accepts, sizing its pre-allocated state. Ignored by
    /// every other kind.
    pub partitions: Option<usize>,
    /// `PV_Kernel`: the compiled magnitude / phase bin-expression programs
    /// (validated at def compile time from the spec's `mag_expr`/`phase_expr`
    /// token lists — see `clausters_core::pvprog`). `None` means the identity.
    /// Ignored by every other kind.
    pub mag_prog: Option<clausters_core::pvprog::PvProgram>,
    pub phase_prog: Option<clausters_core::pvprog::PvProgram>,
    /// Delay family (U3): the longest delay this instance accepts, in
    /// **seconds**. It sizes the pre-allocated line, so like `partitions` it is
    /// static config resolved at build time, not a signal input. Ignored by
    /// every other kind.
    pub max_delay: Option<f32>,
}

/// What the engine knows at **build** time, handed to every
/// [`UGenDescriptor::build`].
///
/// A UGen whose *allocation* depends on the sample rate — a delay line is
/// `max_delay × sample_rate` samples — cannot compute its size from
/// [`UGenConfig`] alone, and it must allocate here, on the network thread,
/// never in `process`. This is the same information `FaustSynth::new` already
/// takes for the same reason.
#[derive(Clone, Debug)]
pub struct BuildCtx {
    /// The engine's sample rate in Hz, fixed for the server's run.
    pub sample_rate: f32,
    /// The full control block length in samples. A scheduled bundle may split a
    /// block into shorter runs, so this is a **capacity**, not the length any
    /// single `process` call sees — size buffers by it, never loop over it.
    pub block_size: usize,
    /// The next seed for a stochastic UGen in this instance, handed out by
    /// [`BuildCtx::next_seed`]. It is a `Cell` because `build` takes `&self`
    /// and every UGen in one graph must get a *different* stream: correlated
    /// "noise" sums to a comb filter rather than to more noise.
    seed: Cell<u64>,
}

impl BuildCtx {
    /// A build context whose stochastic UGens start their seeds at `seed`.
    ///
    /// The seed sequence belongs to the *instance*, not to the process: the
    /// caller (`UGenSynth::new`) reserves one contiguous run of seeds per
    /// synth, so replaying the same score replays the same noise. A
    /// process-global counter would make a render depend on how many synths
    /// happened to be built before it.
    pub fn new(sample_rate: f32, block_size: usize, seed: u64) -> Self {
        Self {
            sample_rate,
            block_size,
            seed: Cell::new(seed),
        }
    }

    /// The next seed, advancing the sequence.
    pub fn next_seed(&self) -> u64 {
        let s = self.seed.get();
        self.seed.set(s.wrapping_add(SEED_STRIDE));
        s
    }
}

/// Input count of a UGen: a fixed number, or variable (`EnvGen`, `Dseq`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arity {
    Fixed(usize),
    Variadic,
}

/// One named input slot of a UGen, in **wire order** — the position it occupies
/// in the def's `inputs` array.
///
/// The wire itself stays positional (a def names a `kind` and lists values; no
/// input is ever addressed by name), so this is descriptive metadata, not a new
/// contract: it exists so `/u_query` can report a UGen's signature and a client
/// palette can label an inlet instead of copying the names into its own table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UGenInput {
    pub name: &'static str,
    /// The value a client should offer when the user leaves the slot alone.
    /// Advisory: the server applies no default of its own — a def that omits an
    /// input is simply short, and the compiler rejects it by arity.
    pub default: f32,
}

/// A named input slot (keeps the `UGENS` rows readable).
const fn inp(name: &'static str, default: f32) -> UGenInput {
    UGenInput { name, default }
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
    /// Reads the **done flag** of the UGen its first input names, before
    /// running (`Done`, `FreeSelfWhenDone`, U4). Like [`DemandDriver`](Self::
    /// DemandDriver) this needs the input's *identity*, not its value, so the
    /// synth resolves the wire index and the compiler requires a wire there —
    /// a kind whose descriptor sets `has_done_flag`.
    DoneQuery,
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
    /// Combines **two** chains (`PV_Add`/`PV_Mul`/…, M27): inputs 0 and 1 are
    /// chain wires of equal window size and distinct slots; the result lands
    /// in chain A (input 0), whose slot the combiner inherits.
    Filter2,
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
    /// The named input slots, in wire order. For a [`Arity::Fixed`] kind this
    /// covers every slot; for a [`Arity::Variadic`] one it names only the
    /// **fixed head** (`EnvGen`'s five before the envelope array, `Dseq`'s
    /// `repeats`), the tail being an unbounded run of like-typed values.
    pub inputs: &'static [UGenInput],
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
    /// Whether this kind raises a **done flag** when it finishes, i.e. whether
    /// [`UGen::is_done`] can ever be true for it.
    /// `Done`/`FreeSelfWhenDone` may only watch a kind that does; the compiler
    /// rejects the rest by name, since watching a UGen that never finishes
    /// would read zero for the life of the node with nothing to see.
    pub has_done_flag: bool,
    /// Builds an instance. Runs on the network thread (allocates); `config`
    /// carries static per-UGen parameters, ignored by most kinds, and `ctx` the
    /// engine facts a kind may need to size that allocation (the sample rate,
    /// for a delay line). Most kinds ignore both.
    pub build: fn(&UGenConfig, &BuildCtx) -> Box<dyn UGen>,
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
    inputs: &'static [UGenInput],
    default_rate: Rate,
    rates: &'static [Rate],
    exec: ExecMode,
    bus: BusRole,
    needs_path: bool,
    op_family: Option<OpFamily>,
    spectral: SpectralRole,
    build: fn(&UGenConfig, &BuildCtx) -> Box<dyn UGen>,
) -> UGenDescriptor {
    UGenDescriptor {
        name,
        arity,
        inputs,
        default_rate,
        rates,
        exec,
        bus,
        needs_path,
        op_family,
        spectral,
        build,
        has_done_flag: false,
    }
}

/// A kind that raises a done flag when it finishes (the envelope family), so
/// `Done`/`FreeSelfWhenDone` may watch it. Wraps [`desc`] rather than widening
/// its argument list, since three rows out of a hundred want this.
const fn desc_done(d: UGenDescriptor) -> UGenDescriptor {
    UGenDescriptor {
        has_done_flag: true,
        ..d
    }
}

/// Compact descriptor constructor (keeps `UGENS` readable as a table): a plain
/// fixed-behavior kind, no `op` family.
#[allow(clippy::too_many_arguments)]
const fn desc(
    name: &'static str,
    arity: Arity,
    inputs: &'static [UGenInput],
    default_rate: Rate,
    rates: &'static [Rate],
    exec: ExecMode,
    bus: BusRole,
    needs_path: bool,
    build: fn(&UGenConfig, &BuildCtx) -> Box<dyn UGen>,
) -> UGenDescriptor {
    desc_full(
        name,
        arity,
        inputs,
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
    inputs: &'static [UGenInput],
    default_rate: Rate,
    rates: &'static [Rate],
    role: SpectralRole,
    build: fn(&UGenConfig, &BuildCtx) -> Box<dyn UGen>,
) -> UGenDescriptor {
    desc_full(
        name,
        arity,
        inputs,
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
    inputs: &'static [UGenInput],
    family: OpFamily,
    build: fn(&UGenConfig, &BuildCtx) -> Box<dyn UGen>,
) -> UGenDescriptor {
    desc_full(
        name,
        arity,
        inputs,
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
use ExecMode::{DemandDriver, DoneQuery, LocalIn as ExecLocalIn, LocalOut as ExecLocalOut, Normal};
use Rate::{Ar, Dr, Ir, Kr};

// Input signatures shared by several rows. Named in **wire order** and in
// `snake_case`, the one style the whole surface uses (the Python callables, the
// catalog table in `docs/schemas.md` and these rows must agree — a client test
// contrasts them, see the M30 note in docs/decisions.md).
const I_NONE: &[UGenInput] = &[];
const I_A: &[UGenInput] = &[inp("a", 0.0)];
const I_AB: &[UGenInput] = &[inp("a", 0.0), inp("b", 0.0)];
const I_ABC: &[UGenInput] = &[inp("a", 0.0), inp("b", 0.0), inp("c", 0.0)];
/// The non-band-limited modulation shapes (U1). The two with a duty cycle
/// declare a third input; the two without do not, so `/u_query` never reports
/// an inlet the UGen ignores.
const I_LF: &[UGenInput] = &[inp("freq", 440.0), inp("iphase", 0.0)];
const I_LF_WIDTH: &[UGenInput] = &[inp("freq", 440.0), inp("iphase", 0.0), inp("width", 0.5)];
/// The one-segment ramps (U4). `start`, `end` and `dur` are read once, on the
/// first sample, as scsynth reads them: the ramp's geometry is fixed at birth
/// and modulating these does nothing. `done_action` is the exception — an
/// input rather than static config, read every block, because it says what
/// happens to the *node* and a def may re-aim that mid-flight.
const I_LINE: &[UGenInput] = &[
    inp("start", 0.0),
    inp("end", 1.0),
    inp("dur", 1.0),
    inp("done_action", 0.0),
];
/// `XLine`'s only difference is where it starts: an exponential ramp from zero
/// is degenerate, so the value a client offers must not be one.
const I_XLINE: &[UGenInput] = &[
    inp("start", 0.01),
    inp("end", 1.0),
    inp("dur", 1.0),
    inp("done_action", 0.0),
];
/// The noise family (U6). The three spectral shapes and the two bit sources
/// take no input at all; the held ones take a frequency, and `Dust` a mean
/// density in impulses per second.
const I_LF_NOISE: &[UGenInput] = &[inp("freq", 500.0)];
const I_DENSITY: &[UGenInput] = &[inp("density", 1.0)];
const I_CHAOS: &[UGenInput] = &[inp("chaos", 1.5)];
/// The pan family (U7). Every row that emits two channels ends in `chan`, the
/// index of the one *this* instance carries: the engine gives a UGen one
/// output, so a stereo panner is two rows sharing their inputs, and the Python
/// builder returns the pair as a channel list. It sits last because it is the
/// builder's business, not the reader's — the inputs before it are scsynth's,
/// in scsynth's order.
const I_PAN2: &[UGenInput] = &[
    inp("signal", 0.0),
    inp("pos", 0.0),
    inp("level", 1.0),
    inp("chan", 0.0),
];
const I_BALANCE2: &[UGenInput] = &[
    inp("left", 0.0),
    inp("right", 0.0),
    inp("pos", 0.0),
    inp("level", 1.0),
    inp("chan", 0.0),
];
const I_XFADE2: &[UGenInput] = &[
    inp("a", 0.0),
    inp("b", 0.0),
    inp("pan", 0.0),
    inp("level", 1.0),
];
const I_ROTATE2: &[UGenInput] = &[
    inp("x", 0.0),
    inp("y", 0.0),
    inp("pos", 0.0),
    inp("chan", 0.0),
];
const I_MIDSIDE: &[UGenInput] = &[inp("a", 0.0), inp("b", 0.0), inp("chan", 0.0)];
const I_WIDTH: &[UGenInput] = &[
    inp("left", 0.0),
    inp("right", 0.0),
    inp("width", 1.0),
    inp("chan", 0.0),
];
const I_PAN_AZ: &[UGenInput] = &[
    inp("signal", 0.0),
    inp("pos", 0.0),
    inp("level", 1.0),
    inp("width", 2.0),
    inp("orientation", 0.5),
    inp("numchans", 2.0),
    inp("chan", 0.0),
];
/// `Select`/`SelectX`: the index, then an unbounded run of sources.
const I_WHICH: &[UGenInput] = &[inp("which", 0.0)];
// The demand family (U8). `repeats` leads every source that has one — for a
// list it counts passes, for a random pick it counts items (scsynth's own
// asymmetry, kept). The two stochastic shapes differ by the walk's `step`
// alone. Both drivers put their clock first; `gap_first` is `TDuty`'s only.
const I_REPEATS: &[UGenInput] = &[inp("repeats", 0.0)];
const I_DRAW: &[UGenInput] = &[inp("repeats", 0.0), inp("lo", 0.0), inp("hi", 1.0)];
const I_DWALK: &[UGenInput] = &[
    inp("repeats", 0.0),
    inp("lo", 0.0),
    inp("hi", 1.0),
    inp("step", 0.01),
];
const I_DUTY: &[UGenInput] = &[
    inp("dur", 1.0),
    inp("reset", 0.0),
    inp("level", 1.0),
    inp("done_action", 0.0),
];
const I_TDUTY: &[UGenInput] = &[
    inp("dur", 1.0),
    inp("reset", 0.0),
    inp("level", 1.0),
    inp("done_action", 0.0),
    inp("gap_first", 0.0),
];
/// The trigger family (U5). A kind that takes only triggers has no signal
/// input at all — but it still defaults to `ar`, because a `kr` consumer
/// samples an `ar` wire once per block and would drop most of a trigger train.
const I_TRIG_DUR: &[UGenInput] = &[inp("signal", 0.0), inp("dur", 0.1)];
const I_HOLD: &[UGenInput] = &[inp("signal", 0.0), inp("trig", 0.0)];
const I_SCHMIDT: &[UGenInput] = &[inp("signal", 0.0), inp("lo", 0.0), inp("hi", 1.0)];
const I_TRIG: &[UGenInput] = &[inp("trig", 0.0)];
const I_TRIG_RESET: &[UGenInput] = &[inp("trig", 0.0), inp("reset", 0.0)];
const I_DIVIDER: &[UGenInput] = &[inp("trig", 0.0), inp("div", 2.0), inp("start", 0.0)];
const I_STEPPER: &[UGenInput] = &[
    inp("trig", 0.0),
    inp("reset", 0.0),
    inp("min", 0.0),
    inp("max", 7.0),
    inp("step", 1.0),
    inp("resetval", 0.0),
];
const I_SWEEP: &[UGenInput] = &[inp("trig", 0.0), inp("rate", 1.0)];
const I_CHANGED: &[UGenInput] = &[inp("signal", 0.0), inp("threshold", 0.0)];
const I_DECAY: &[UGenInput] = &[inp("signal", 0.0), inp("decaytime", 1.0)];
const I_DECAY2: &[UGenInput] = &[
    inp("signal", 0.0),
    inp("attacktime", 0.01),
    inp("decaytime", 1.0),
];
const I_SILENCE: &[UGenInput] = &[
    inp("signal", 0.0),
    inp("amp", 0.0001),
    inp("time", 0.1),
    inp("done_action", 0.0),
];
/// The node-control rows (U4): `FreeSelf`/`PauseSelf` watch a signal,
/// `Done`/`FreeSelfWhenDone` watch the UGen wired into `source`. The names
/// differ because what they read differs — one is a value, the other an
/// identity.
const I_SIGNAL: &[UGenInput] = &[inp("signal", 0.0)];
const I_SOURCE: &[UGenInput] = &[inp("source", 0.0)];
/// The two-pole rows (U2). The Butterworth pair fixes its own damping and
/// therefore has no `rq` wire; the resonant ones read it.
const I_FILT: &[UGenInput] = &[inp("signal", 0.0), inp("freq", 440.0)];
const I_FILT_RQ: &[UGenInput] = &[inp("signal", 0.0), inp("freq", 440.0), inp("rq", 1.0)];
/// The delay family (U3). `max_delay` is static config, not an input: it sizes
/// the allocation, so it belongs where `fft_size` and `partitions` are.
const I_DELAY: &[UGenInput] = &[inp("signal", 0.0), inp("delaytime", 0.2)];
const I_DELAY_DECAY: &[UGenInput] = &[
    inp("signal", 0.0),
    inp("delaytime", 0.2),
    inp("decaytime", 1.0),
];
/// The one-pole family takes a pole coefficient, not a frequency. The two
/// whose job fixes where that pole belongs offer their own value: a DC blocker
/// and a leaky integrator both want a pole just inside the unit circle, and 0.5
/// would make one deaf to bass and the other forget almost immediately.
const I_ONE: &[UGenInput] = &[inp("signal", 0.0), inp("coef", 0.5)];
const I_LEAK: &[UGenInput] = &[inp("signal", 0.0), inp("coef", 0.995)];
const I_INTEGRATE: &[UGenInput] = &[inp("signal", 0.0), inp("coef", 0.999)];
const I_BUS: &[UGenInput] = &[inp("bus", 0.0)];
const I_BUS_SIGNAL: &[UGenInput] = &[inp("bus", 0.0), inp("signal", 0.0)];
const I_BUFNUM: &[UGenInput] = &[inp("bufnum", 0.0)];
const I_TABLE_OSC: &[UGenInput] = &[inp("bufnum", 0.0), inp("freq", 440.0), inp("phase", 0.0)];
const I_CHAIN: &[UGenInput] = &[inp("chain", 0.0)];
const I_CHAIN_AB: &[UGenInput] = &[inp("chain_a", 0.0), inp("chain_b", 0.0)];
const I_CHAIN_THRESHOLD: &[UGenInput] = &[inp("chain", 0.0), inp("threshold", 0.0)];
const I_CHAIN_SHIFT: &[UGenInput] = &[inp("chain", 0.0), inp("stretch", 1.0), inp("shift", 0.0)];

/// The UGen catalog. **To add a UGen, add one row here** (plus its `dsp`
/// module) — the compiler and bus analysis pick it up with no other change.
static UGENS: &[UGenDescriptor] = &[
    // --- generators (audio or control rate; the default shape) ---
    desc(
        "Sine",
        Fixed(1),
        &[inp("freq", 440.0)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Sine::new()),
    ),
    desc(
        "Impulse",
        Fixed(1),
        &[inp("freq", 1.0)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Impulse::new()),
    ),
    desc(
        "WhiteNoise",
        Fixed(0),
        I_NONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(WhiteNoise::with_seed(ctx.next_seed())),
    ),
    // --- the phase family (U1): one f64 accumulator, PolyBLEP where the
    //     waveform is meant to be heard and exact corners where it is meant to
    //     modulate. See `dsp::phase` for why this is not scsynth's impulse
    //     train. ---
    desc(
        "Saw",
        Fixed(1),
        &[inp("freq", 440.0)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Saw::new()),
    ),
    desc(
        "Pulse",
        Fixed(2),
        &[inp("freq", 440.0), inp("width", 0.5)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Pulse::new()),
    ),
    desc(
        "LFSaw",
        Fixed(2),
        I_LF,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Lf::new(LfShape::Saw)),
    ),
    desc(
        "LFPulse",
        Fixed(3),
        I_LF_WIDTH,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Lf::new(LfShape::Pulse)),
    ),
    desc(
        "LFTri",
        Fixed(2),
        I_LF,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Lf::new(LfShape::Tri)),
    ),
    desc(
        "VarSaw",
        Fixed(3),
        I_LF_WIDTH,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Lf::new(LfShape::VarSaw)),
    ),
    desc(
        "Phasor",
        Fixed(5),
        &[
            inp("trig", 0.0),
            inp("rate", 1.0),
            inp("start", 0.0),
            inp("end", 1.0),
            inp("reset_pos", 0.0),
        ],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Phasor::new()),
    ),
    // --- the filter core (U2): one topology-preserving state-variable
    //     implementation behind every two-pole name, plus the one-pole family.
    //     `BPF` and `Resonz` are the same row twice on purpose - scsynth ships
    //     two historically distinct resonators with the same parameterization
    //     and the same unity peak gain. See `dsp::filter`. ---
    desc(
        "LPF",
        Fixed(2),
        I_FILT,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Lp)),
    ),
    desc(
        "HPF",
        Fixed(2),
        I_FILT,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Hp)),
    ),
    desc(
        "RLPF",
        Fixed(3),
        I_FILT_RQ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::RLp)),
    ),
    desc(
        "RHPF",
        Fixed(3),
        I_FILT_RQ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::RHp)),
    ),
    desc(
        "BPF",
        Fixed(3),
        I_FILT_RQ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Bp)),
    ),
    desc(
        "Resonz",
        Fixed(3),
        I_FILT_RQ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Bp)),
    ),
    desc(
        "BRF",
        Fixed(3),
        I_FILT_RQ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Notch)),
    ),
    desc(
        "Svf",
        Fixed(6),
        &[
            inp("signal", 0.0),
            inp("freq", 440.0),
            inp("rq", 1.0),
            inp("low", 1.0),
            inp("band", 0.0),
            inp("high", 0.0),
        ],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Mix)),
    ),
    desc(
        "OnePole",
        Fixed(2),
        I_ONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OneFilter::new(OneKind::OnePole)),
    ),
    desc(
        "OneZero",
        Fixed(2),
        I_ONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OneFilter::new(OneKind::OneZero)),
    ),
    desc(
        "LeakDC",
        Fixed(2),
        I_LEAK,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OneFilter::new(OneKind::LeakDc)),
    ),
    desc(
        "Integrator",
        Fixed(2),
        I_INTEGRATE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OneFilter::new(OneKind::Integrator)),
    ),
    // --- the delay core (U3): one circular line, parameterized by
    //     interpolation and by what it feeds back. The line is synth-private
    //     memory sized at build from `max_delay` and the sample rate - a pool
    //     buffer is immutable, and this one is written every sample. See
    //     `dsp::delay`. ---
    desc(
        "DelayN",
        Fixed(2),
        I_DELAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::None,
                Feedback::None,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "CombN",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::None,
                Feedback::Comb,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "AllpassN",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::None,
                Feedback::Allpass,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "DelayL",
        Fixed(2),
        I_DELAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Linear,
                Feedback::None,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "CombL",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Linear,
                Feedback::Comb,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "AllpassL",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Linear,
                Feedback::Allpass,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "DelayC",
        Fixed(2),
        I_DELAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Cubic,
                Feedback::None,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "CombC",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Cubic,
                Feedback::Comb,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "AllpassC",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Cubic,
                Feedback::Allpass,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    // --- arithmetic: the generic op UGens (S3), selected by a core opcode
    //     index; every math need is one more `clausters_core::builtins` entry,
    //     not a new kind. `Add`/`Sub`/`Mul`/`Div` stay as thin aliases below
    //     for back-compat with existing defs. ---
    desc_op("BinaryOpUGen", Fixed(2), I_AB, OpFamily::Binary, |c, _| {
        Box::new(BinaryOp::from_index(c.op.unwrap_or(0)))
    }),
    desc_op("UnaryOpUGen", Fixed(1), I_A, OpFamily::Unary, |c, _| {
        Box::new(UnaryOp::from_index(c.op.unwrap_or(0)))
    }),
    // Fused forms scsynth optimizes (fixed kinds, not op-table entries).
    desc(
        "MulAdd",
        Fixed(3),
        I_ABC,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(MulAdd),
    ),
    desc(
        "Sum3",
        Fixed(3),
        I_ABC,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Sum3),
    ),
    desc(
        "Sum4",
        Fixed(4),
        &[inp("a", 0.0), inp("b", 0.0), inp("c", 0.0), inp("d", 0.0)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Sum4),
    ),
    // Aliases for the four operator kinds (back-compat).
    desc(
        "Add",
        Fixed(2),
        I_AB,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BinaryOp::new(BinOp::Add)),
    ),
    desc(
        "Sub",
        Fixed(2),
        I_AB,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BinaryOp::new(BinOp::Sub)),
    ),
    desc(
        "Mul",
        Fixed(2),
        I_AB,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BinaryOp::new(BinOp::Mul)),
    ),
    desc(
        "Div",
        Fixed(2),
        I_AB,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BinaryOp::new(BinOp::Div)),
    ),
    // --- audio-bus I/O (audio rate only; carries a bus role) ---
    desc(
        "In",
        Fixed(1),
        I_BUS,
        Ar,
        R_AR,
        Normal,
        Read,
        false,
        |_, _| Box::new(In),
    ),
    desc(
        "InCtl",
        Fixed(1),
        I_BUS,
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(InCtl),
    ),
    desc(
        "OutCtl",
        Fixed(2),
        I_BUS_SIGNAL,
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OutCtl),
    ),
    desc(
        "Out",
        Fixed(2),
        I_BUS_SIGNAL,
        Ar,
        R_AR,
        Normal,
        Write,
        false,
        |_, _| Box::new(Out),
    ),
    desc(
        "ReplaceOut",
        Fixed(2),
        I_BUS_SIGNAL,
        Ar,
        R_AR,
        Normal,
        ReadWrite,
        false,
        |_, _| Box::new(ReplaceOut),
    ),
    // --- buffer readers and info ---
    desc(
        "PlayBuf",
        Fixed(4),
        &[
            inp("bufnum", 0.0),
            inp("chan", 0.0),
            inp("rate", 1.0),
            inp("loop", 0.0),
        ],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(PlayBuf::new()),
    ),
    desc(
        "BufRd",
        Fixed(4),
        &[
            inp("bufnum", 0.0),
            inp("chan", 0.0),
            inp("phase", 0.0),
            inp("loop", 0.0),
        ],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BufRd),
    ),
    // --- table oscillators & waveshaper (S5); read `/b_gen` wavetables ---
    desc(
        "Osc",
        Fixed(3),
        I_TABLE_OSC,
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Osc::new()),
    ),
    desc(
        "OscN",
        Fixed(3),
        I_TABLE_OSC,
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OscN::new()),
    ),
    desc(
        "VOsc",
        Fixed(3),
        &[inp("bufpos", 0.0), inp("freq", 440.0), inp("phase", 0.0)],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(VOsc::new()),
    ),
    desc(
        "Shaper",
        Fixed(2),
        &[inp("bufnum", 0.0), inp("signal", 0.0)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Shaper),
    ),
    desc(
        "BufSampleRate",
        Fixed(1),
        I_BUFNUM,
        Ar,
        R_IR_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BufInfo(BufInfoKind::SampleRate)),
    ),
    desc(
        "BufRateScale",
        Fixed(1),
        I_BUFNUM,
        Ar,
        R_IR_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BufInfo(BufInfoKind::RateScale)),
    ),
    desc(
        "BufFrames",
        Fixed(1),
        I_BUFNUM,
        Ar,
        R_IR_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BufInfo(BufInfoKind::Frames)),
    ),
    desc(
        "BufChannels",
        Fixed(1),
        I_BUFNUM,
        Ar,
        R_IR_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BufInfo(BufInfoKind::Channels)),
    ),
    desc(
        "BufDur",
        Fixed(1),
        I_BUFNUM,
        Ar,
        R_IR_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BufInfo(BufInfoKind::Duration)),
    ),
    // --- streaming disk I/O (need a path; native only — see dsp::disk) ---
    #[cfg(not(target_arch = "wasm32"))]
    desc(
        "DiskIn",
        Fixed(1),
        &[inp("chan", 0.0)],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        true,
        |c, _| Box::new(DiskIn::open(c)),
    ),
    #[cfg(not(target_arch = "wasm32"))]
    desc(
        "DiskOut",
        Fixed(1),
        &[inp("signal", 0.0)],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        true,
        |c, _| Box::new(DiskOut::open(c)),
    ),
    // --- synth-private feedback (synth-coordinated execution) ---
    desc(
        "LocalIn",
        Fixed(1),
        &[inp("channel", 0.0)],
        Ar,
        R_AR,
        ExecLocalIn,
        BusRole::None,
        false,
        |_, _| Box::new(LocalIn),
    ),
    desc(
        "LocalOut",
        Fixed(2),
        &[inp("channel", 0.0), inp("signal", 0.0)],
        Ar,
        R_AR,
        ExecLocalOut,
        BusRole::None,
        false,
        |_, _| Box::new(LocalOut),
    ),
    // --- one-pole smoothers (also inserted by S2 lagged controls) ---
    desc(
        "Lag",
        Fixed(2),
        &[inp("signal", 0.0), inp("time", 0.1)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Lag::new()),
    ),
    desc(
        "VarLag",
        Fixed(3),
        &[inp("signal", 0.0), inp("up", 0.1), inp("down", 0.1)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(VarLag::new()),
    ),
    // --- envelope ---
    // Variadic: the five named slots are the fixed head; the envelope's own
    // breakpoint array follows and is unnamed by nature.
    desc_done(desc(
        "EnvGen",
        Variadic,
        &[
            inp("gate", 1.0),
            inp("level_scale", 1.0),
            inp("level_bias", 0.0),
            inp("time_scale", 1.0),
            inp("done_action", 0.0),
        ],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(EnvGen::new()),
    )),
    // The one-segment envelopes (U4): the same engine with its header filled
    // in, so they inherit the shape arithmetic and the whole done-action set.
    // Unlike `EnvGen` they run at either rate — a ramp is the archetypal `kr`
    // UGen, and a `kr` one costs a block's worth of work per block.
    desc_done(desc(
        "Line",
        Fixed(4),
        I_LINE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Line::new(LineShape::Linear)),
    )),
    desc_done(desc(
        "XLine",
        Fixed(4),
        I_XLINE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Line::new(LineShape::Exponential)),
    )),
    // --- noise (U6) ---
    desc(
        "PinkNoise",
        Fixed(0),
        I_NONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(PinkNoise::with_seed(ctx.next_seed())),
    ),
    desc(
        "BrownNoise",
        Fixed(0),
        I_NONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(BrownNoise::with_seed(ctx.next_seed())),
    ),
    desc(
        "GrayNoise",
        Fixed(0),
        I_NONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(GrayNoise::with_seed(ctx.next_seed())),
    ),
    desc(
        "ClipNoise",
        Fixed(0),
        I_NONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(ClipNoise::with_seed(ctx.next_seed())),
    ),
    desc(
        "LFNoise0",
        Fixed(1),
        I_LF_NOISE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(LfNoise::with_seed(LfNoiseShape::Step, ctx.next_seed())),
    ),
    desc(
        "LFNoise1",
        Fixed(1),
        I_LF_NOISE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(LfNoise::with_seed(LfNoiseShape::Linear, ctx.next_seed())),
    ),
    desc(
        "LFNoise2",
        Fixed(1),
        I_LF_NOISE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(LfNoise::with_seed(LfNoiseShape::Quadratic, ctx.next_seed())),
    ),
    desc(
        "LFClipNoise",
        Fixed(1),
        I_LF_NOISE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(LfNoise::with_seed(LfNoiseShape::Clip, ctx.next_seed())),
    ),
    desc(
        "Dust",
        Fixed(1),
        I_DENSITY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Dust::with_seed(DustMode::Unipolar, ctx.next_seed())),
    ),
    desc(
        "Dust2",
        Fixed(1),
        I_DENSITY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Dust::with_seed(DustMode::Bipolar, ctx.next_seed())),
    ),
    desc(
        "Crackle",
        Fixed(1),
        I_CHAOS,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Crackle::default()),
    ),
    // --- panning, the stereo field and selection (U7): one pan law, one
    //     two-by-two matrix, one crossfade. The two-channel rows appear once
    //     each and carry a `chan` index; `numchans` on `PanAz` sizes the ring
    //     the gain is computed against, not an allocation. ---
    desc(
        "Pan2",
        Fixed(4),
        I_PAN2,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Pan::new(PanKind::Pan2)),
    ),
    desc(
        "LinPan2",
        Fixed(4),
        I_PAN2,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Pan::new(PanKind::LinPan2)),
    ),
    desc(
        "Balance2",
        Fixed(5),
        I_BALANCE2,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Pan::new(PanKind::Balance2)),
    ),
    desc(
        "XFade2",
        Fixed(4),
        I_XFADE2,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Pan::new(PanKind::XFade2)),
    ),
    desc(
        "LinXFade2",
        Fixed(4),
        I_XFADE2,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Pan::new(PanKind::LinXFade2)),
    ),
    desc(
        "Rotate2",
        Fixed(4),
        I_ROTATE2,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Rotate::new(RotKind::Rotate2)),
    ),
    desc(
        "MidSide",
        Fixed(3),
        I_MIDSIDE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Rotate::new(RotKind::MidSide)),
    ),
    desc(
        "StereoWidth",
        Fixed(4),
        I_WIDTH,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Rotate::new(RotKind::Width)),
    ),
    desc(
        "PanAz",
        Fixed(7),
        I_PAN_AZ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(PanAz),
    ),
    desc(
        "Select",
        Variadic,
        I_WHICH,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Select::new(SelectKind::Pick)),
    ),
    desc(
        "SelectX",
        Variadic,
        I_WHICH,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Select::new(SelectKind::Cross)),
    ),
    // --- triggers and control (U5) ---
    // All of these default to `ar`, counters included. A `kr` UGen reads one
    // sample per block from an `ar` input, so a `kr` counter fed an `ar`
    // trigger train would silently miss 63 triggers out of 64; the cheaper
    // pairing has to be asked for, and asked for on both ends.
    desc(
        "Trig",
        Fixed(2),
        I_TRIG_DUR,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(TrigPulse::new(TrigMode::Level)),
    ),
    desc(
        "Trig1",
        Fixed(2),
        I_TRIG_DUR,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(TrigPulse::new(TrigMode::Unit)),
    ),
    desc(
        "TDelay",
        Fixed(2),
        I_TRIG_DUR,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(TrigPulse::new(TrigMode::Delay)),
    ),
    desc(
        "Latch",
        Fixed(2),
        I_HOLD,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Hold::new(HoldMode::Latch)),
    ),
    desc(
        "Gate",
        Fixed(2),
        I_HOLD,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Hold::new(HoldMode::Gate)),
    ),
    desc(
        "Schmidt",
        Fixed(3),
        I_SCHMIDT,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Schmidt::default()),
    ),
    desc(
        "ToggleFF",
        Fixed(1),
        I_TRIG,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(FlipFlop::new(FlipFlopMode::Toggle)),
    ),
    desc(
        "SetResetFF",
        Fixed(2),
        I_TRIG_RESET,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(FlipFlop::new(FlipFlopMode::SetReset)),
    ),
    desc(
        "PulseCount",
        Fixed(2),
        I_TRIG_RESET,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Counter::new(CounterMode::Count)),
    ),
    desc(
        "PulseDivider",
        Fixed(3),
        I_DIVIDER,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Counter::new(CounterMode::Divide)),
    ),
    desc(
        "Stepper",
        Fixed(6),
        I_STEPPER,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Stepper::default()),
    ),
    desc(
        "Timer",
        Fixed(1),
        I_TRIG,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Elapsed::new(ElapsedMode::Timer)),
    ),
    desc(
        "Sweep",
        Fixed(2),
        I_SWEEP,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Elapsed::new(ElapsedMode::Sweep)),
    ),
    desc(
        "Changed",
        Fixed(2),
        I_CHANGED,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Changed::default()),
    ),
    desc(
        "Decay",
        Fixed(2),
        I_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Decay::new(false)),
    ),
    desc(
        "Decay2",
        Fixed(3),
        I_DECAY2,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Decay::new(true)),
    ),
    // The one row here with a done flag: it exists to notice that a voice has
    // nothing left to say, so `Done`/`FreeSelfWhenDone` can watch it too.
    desc_done(desc(
        "DetectSilence",
        Fixed(4),
        I_SILENCE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(DetectSilence::default()),
    )),
    // --- node control (U4) ---
    // These four pass their input through and act on their own node. The first
    // two read a signal; the last two read another UGen's done flag, which is
    // why they run in `DoneQuery` rather than `Normal`.
    desc(
        "FreeSelf",
        Fixed(1),
        I_SIGNAL,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(SelfControl::new(DoneAction::FreeSelf)),
    ),
    desc(
        "PauseSelf",
        Fixed(1),
        I_SIGNAL,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(SelfControl::new(DoneAction::PauseSelf)),
    ),
    desc(
        "Done",
        Fixed(1),
        I_SOURCE,
        Ar,
        R_KR_AR,
        DoneQuery,
        BusRole::None,
        false,
        |_, _| Box::new(WhenDone::new(WhenDoneMode::Report)),
    ),
    desc(
        "FreeSelfWhenDone",
        Fixed(1),
        I_SOURCE,
        Ar,
        R_KR_AR,
        DoneQuery,
        BusRole::None,
        false,
        |_, _| Box::new(WhenDone::new(WhenDoneMode::Free)),
    ),
    // --- scalar / init-rate (S1) ---
    desc(
        "SampleRate",
        Fixed(0),
        I_NONE,
        Ir,
        R_IR_KR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(SampleRate),
    ),
    desc(
        "Rand",
        Fixed(2),
        &[inp("lo", 0.0), inp("hi", 1.0)],
        Ir,
        R_IR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Rand::new()),
    ),
    // --- demand rate (S1 substrate, U8 catalog): a driver runs specially and
    //     pulls; a source is skipped in block order and reached only through a
    //     pull, its own inputs pulled with it when they are streams too. ---
    desc(
        "Demand",
        Fixed(3),
        &[inp("trig", 0.0), inp("reset", 0.0), inp("source", 0.0)],
        Ar,
        R_KR_AR,
        DemandDriver,
        BusRole::None,
        false,
        |_, _| Box::new(Demand::new()),
    ),
    desc_done(desc(
        "Duty",
        Fixed(4),
        I_DUTY,
        Ar,
        R_KR_AR,
        DemandDriver,
        BusRole::None,
        false,
        |_, _| Box::new(Duty::new(DutyKind::Hold)),
    )),
    desc_done(desc(
        "TDuty",
        Fixed(5),
        I_TDUTY,
        Ar,
        R_KR_AR,
        DemandDriver,
        BusRole::None,
        false,
        |_, _| Box::new(Duty::new(DutyKind::Trigger)),
    )),
    desc(
        "Dseq",
        Variadic,
        I_REPEATS,
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Dlist::with_seed(ListOrder::Seq, ctx.next_seed())),
    ),
    desc(
        "Drand",
        Variadic,
        I_REPEATS,
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Dlist::with_seed(ListOrder::Rand, ctx.next_seed())),
    ),
    desc(
        "Dxrand",
        Variadic,
        I_REPEATS,
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Dlist::with_seed(ListOrder::Xrand, ctx.next_seed())),
    ),
    desc(
        "Dshuf",
        Variadic,
        I_REPEATS,
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Dlist::with_seed(ListOrder::Shuf, ctx.next_seed())),
    ),
    desc(
        "Dseries",
        Fixed(3),
        &[inp("repeats", 0.0), inp("start", 0.0), inp("step", 1.0)],
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Dramp::new(RampKind::Series)),
    ),
    desc(
        "Dgeom",
        Fixed(3),
        &[inp("repeats", 0.0), inp("start", 1.0), inp("grow", 2.0)],
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Dramp::new(RampKind::Geom)),
    ),
    desc(
        "Dwhite",
        Fixed(3),
        I_DRAW,
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Drandom::with_seed(RandKind::White, ctx.next_seed())),
    ),
    desc(
        "Diwhite",
        Fixed(3),
        I_DRAW,
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Drandom::with_seed(RandKind::IWhite, ctx.next_seed())),
    ),
    desc(
        "Dbrown",
        Fixed(4),
        I_DWALK,
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Drandom::with_seed(RandKind::Brown, ctx.next_seed())),
    ),
    desc(
        "Dibrown",
        Fixed(4),
        // Same shape as `I_DWALK`, but a walk over the integers whose default
        // step was a hundredth would never move.
        &[
            inp("repeats", 0.0),
            inp("lo", 0.0),
            inp("hi", 1.0),
            inp("step", 1.0),
        ],
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Drandom::with_seed(RandKind::IBrown, ctx.next_seed())),
    ),
    desc(
        "Dstutter",
        Fixed(2),
        &[inp("repeats", 1.0), inp("value", 0.0)],
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Dstutter::new()),
    ),
    desc(
        "Dswitch1",
        Variadic,
        I_WHICH,
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Dswitch1),
    ),
    desc(
        "Dbufrd",
        Fixed(4),
        &[
            inp("bufnum", 0.0),
            inp("phase", 0.0),
            inp("loop", 1.0),
            inp("channel", 0.0),
        ],
        Dr,
        R_DR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Dbufrd),
    ),
    // --- side-effect UGens (S9): reply/observe, no `Out` required. Control or
    //     audio rate; their output is silence (SendTrig/SendReply) or the
    //     polled signal passed through (Poll). ---
    desc(
        "SendTrig",
        Fixed(3),
        &[inp("trig", 0.0), inp("id", 0.0), inp("value", 0.0)],
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(SendTrig::new()),
    ),
    // Variadic: `trig`/`reply_id` are the head, the reported values follow.
    desc(
        "SendReply",
        Variadic,
        &[inp("trig", 0.0), inp("reply_id", -1.0)],
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, _| Box::new(SendReply::new(c)),
    ),
    desc(
        "Poll",
        Fixed(3),
        &[inp("trig", 0.0), inp("signal", 0.0), inp("trig_id", -1.0)],
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, _| Box::new(Poll::new(c)),
    ),
    // --- frequency-domain (`fr`) chain (S8): FFT opens a synth-private
    //     spectral chain, PV_* transform it in place, IFFT resynthesises audio.
    //     FFT/PV carry the chain at control rate (a per-block ready marker);
    //     IFFT produces audio. See `dsp::spectral`. ---
    desc_spectral(
        "FFT",
        Fixed(2),
        &[inp("source", 0.0), inp("active", 1.0)],
        Kr,
        R_KR,
        SpectralRole::Source,
        |c, _| Box::new(Fft::new(c)),
    ),
    desc_spectral(
        "IFFT",
        Fixed(1),
        I_CHAIN,
        Ar,
        R_AR,
        SpectralRole::Sink,
        |c, _| Box::new(Ifft::new(c)),
    ),
    desc_spectral(
        "PV_MagAbove",
        Fixed(2),
        I_CHAIN_THRESHOLD,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_, _| Box::new(PvMag::new(MagMode::Above)),
    ),
    desc_spectral(
        "PV_MagBelow",
        Fixed(2),
        I_CHAIN_THRESHOLD,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_, _| Box::new(PvMag::new(MagMode::Below)),
    ),
    desc_spectral(
        "PV_BrickWall",
        Fixed(2),
        &[inp("chain", 0.0), inp("wipe", 0.0)],
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_, _| Box::new(PvBrickWall),
    ),
    // M27: the curated PV set — parameterized implementations under the
    // scsynth-compatible names, deliberately not a one-UGen-per-op catalog
    // (see docs/decisions.md). PvMag gains a clip mode; PvCombine is ONE
    // binary implementation behind six names; the stateful pair and the bin
    // remap allocate their scratch at build (network thread).
    desc_spectral(
        "PV_MagClip",
        Fixed(2),
        I_CHAIN_THRESHOLD,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_, _| Box::new(PvMag::new(MagMode::Clip)),
    ),
    desc_spectral(
        "PV_Add",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::Add)),
    ),
    desc_spectral(
        "PV_Mul",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::Mul)),
    ),
    desc_spectral(
        "PV_Min",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::Min)),
    ),
    desc_spectral(
        "PV_Max",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::Max)),
    ),
    desc_spectral(
        "PV_MagMul",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::MagMul)),
    ),
    desc_spectral(
        "PV_CopyPhase",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::CopyPhase)),
    ),
    desc_spectral(
        "PV_MagFreeze",
        Fixed(2),
        &[inp("chain", 0.0), inp("freeze", 0.0)],
        Kr,
        R_KR,
        SpectralRole::Filter,
        |c, _| Box::new(PvMagFreeze::new(c)),
    ),
    desc_spectral(
        "PV_MagSmear",
        Fixed(2),
        &[inp("chain", 0.0), inp("bins", 0.0)],
        Kr,
        R_KR,
        SpectralRole::Filter,
        |c, _| Box::new(PvMagSmear::new(c)),
    ),
    desc_spectral(
        "PV_BinShift",
        Fixed(3),
        I_CHAIN_SHIFT,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |c, _| Box::new(PvBinShift::new(c, false)),
    ),
    desc_spectral(
        "PV_MagShift",
        Fixed(3),
        I_CHAIN_SHIFT,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |c, _| Box::new(PvBinShift::new(c, true)),
    ),
    // M29: the general per-frame mechanism — one UGen interpreting a
    // compile-validated bin-expression program (`mag_expr`/`phase_expr`) over
    // every bin of each fresh frame. Inputs: `[chain, p0, p1, …]` (variadic
    // parameters, sampled at the hop). An op outside the curated set is a
    // program here, never a new registry row (see docs/decisions.md).
    // Variadic: only `chain` is fixed; `p0…` are the program's parameters.
    desc_spectral(
        "PV_Kernel",
        Variadic,
        I_CHAIN,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |c, _| Box::new(PvKernel::new(c)),
    ),
    // --- partitioned convolution (M28): one UGen, kernel spectra prepared
    //     off the RT thread by `/b_gen prepare_partconv`, MACs spread across
    //     the hop's blocks for flat load. Not a PV_*: fast convolution's
    //     zero-padded rectangular segments are incompatible with the windowed
    //     COLA chain. See `dsp::conv`. ---
    desc(
        "Conv",
        Fixed(2),
        &[inp("source", 0.0), inp("kernel", 0.0)],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |c, _| Box::new(Conv::new(c)),
    ),
];

/// Looks a UGen up by its wire name. Returns the descriptor (the single source
/// of truth for that kind) or `None` for an unknown name.
pub fn lookup(name: &str) -> Option<&'static UGenDescriptor> {
    UGENS.iter().find(|d| d.name == name)
}

/// The whole catalog, in table order — what `/u_query` reports (M30). The
/// contents depend on the build (`DiskIn`/`DiskOut` are native-only), which is
/// exactly why a client asks the server instead of carrying its own copy.
pub fn all() -> &'static [UGenDescriptor] {
    UGENS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalog is data, and `/u_query` publishes it: a row whose names do
    /// not line up with its arity would ship a wrong signature to every client
    /// palette. Not feature-gated — the table exists in every build.
    #[test]
    fn every_descriptor_names_its_inputs() {
        for d in all() {
            match d.arity {
                Arity::Fixed(n) => assert_eq!(
                    d.inputs.len(),
                    n,
                    "{} declares arity {n} but names {} inputs",
                    d.name,
                    d.inputs.len()
                ),
                // The named slots are the fixed head, so they can only be
                // fewer than what an instance ends up with -- never a count.
                Arity::Variadic => assert!(
                    !d.inputs.is_empty(),
                    "{} is variadic but names no fixed head",
                    d.name
                ),
            }
            for i in d.inputs {
                assert!(!i.name.is_empty(), "{} has an unnamed input", d.name);
            }
            for (i, a) in d.inputs.iter().enumerate() {
                assert!(
                    !d.inputs[..i].iter().any(|b| b.name == a.name),
                    "{} repeats the input name {:?}",
                    d.name,
                    a.name
                );
            }
        }
    }

    #[test]
    fn catalog_names_are_unique() {
        for (i, d) in all().iter().enumerate() {
            assert!(
                !all()[..i].iter().any(|o| o.name == d.name),
                "duplicate catalog entry {:?}",
                d.name
            );
        }
    }
}
